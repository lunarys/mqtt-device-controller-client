extern crate argparse;

use argparse::{ArgumentParser, StoreTrue, Store, StoreOption};
use paho_mqtt as mqtt;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use std::ops::Add;
use std::process::exit;

macro_rules! try_result {
    ($res:expr, $err:expr) => {
        match $res {
            Ok(val) => val,
            Err(_) => return Err($err.to_string())
        }
    }
}

macro_rules! try_option {
    ($opt:expr, $err:expr) => {
        match $opt {
            Some(val) => val,
            None => return Err($err.to_string())
        }
    }
}

struct Configuration {
    start: bool,
    device: String,
    topic_sub: Option<String>,
    topic_pub: Option<String>,
    timeout: u64
}

struct MqttConfiguration {
    host: String,
    port: i32,
    user: String,
    password: Option<String>,
    qos: i32
}

fn main() {
    let mut operation = String::new();

    let mut config = Configuration {
        start: true,
        device: String::new(),
        topic_sub: None,
        topic_pub: None,
        timeout: 600
    };
    let mut mqtt_config = MqttConfiguration {
        host: String::new(),
        port: 1883,
        user: String::new(),
        password: None,
        qos: 1
    };

    {
        let mut parser = ArgumentParser::new();
        parser.set_description("Client to interact with a MQTT device controller");
        parser.refer(&mut operation)
            .add_argument("operation", Store, "Operation to perform ('begin' or 'end')")
            .required();
        parser.refer(&mut config.device)
            .add_option(&["-d", "--device"], Store, "Name of the device to start")
            .required();
        parser.refer(&mut mqtt_config.host)
            .add_option(&["-h", "--host"], Store, "Hostname of the mqtt broker to connect to")
            .required();
        parser.refer(&mut mqtt_config.port)
            .add_option(&["-p", "--port"], Store, "Port of the mqtt broker to connect to (default=1883)");
        parser.refer(&mut mqtt_config.user)
            .add_option(&["-u", "--user"], Store, "Username for authentication and topic generation")
            .required();
        parser.refer(&mut mqtt_config.password)
            .add_option(&["-P", "--password"], StoreOption, "Password for authentication at the mqtt broker");
        parser.refer(&mut config.timeout)
            .add_option(&["-t", "--timeout"], Store, "Time to wait for a message from the controller (default=600)");
        parser.refer(&mut config.start)
            .add_option(&["--passive", "--no-start"], StoreTrue, "Only check if the device is available, do not start it");
        parser.refer(&mut config.topic_sub)
            .add_option(&["--topic-sub"], StoreOption, "Topic used for subscribing to messages from the controller (default=device/$device/controller/to/$user)");
        parser.refer(&mut config.topic_pub)
            .add_option(&["--topic-pub"], StoreOption, "Topic used for publishing messages to the controller (default=device/$device/controller/from/$user)");
        parser.refer(&mut mqtt_config.qos)
            .add_option(&["-q", "--qos"], Store, "Quallity of service for the communication with the mqtt broker (default=1)");
        parser.parse_args_or_exit();
    }

    let result = match operation.as_str() {
        "begin" => start_device(&config, &mqtt_config),
        "end" => stop_device(&config, &mqtt_config),
        _ => Err(format!("Unknown operation: '{}', expected 'begin' or 'end'", operation))
    };

    if result.is_ok() {
        if result.unwrap() {
            println!("Controller sent '{}' successfully to device '{}'", operation, config.device);
            exit(0);
        } else {
            println!("Device '{}' is not available for sync", config.device);
            exit(10);
        }
    } else {
        eprintln!("Could not run controller: {}", result.unwrap_err());
        exit(1);
    }
}

fn get_controller_state(client: &mqtt::Client, receiver: &Receiver<Option<mqtt::Message>>, config: &Configuration, qos: i32) -> Result<bool, String> {
    let topic = get_controller_state_topic(config);

    // Subscribe to state topic
    try_result!(client.subscribe(topic.as_str(), qos), "Could not subscribe to controller state topic");

    // 10 seconds should be more than enough, as the state is retained
    let wait_time = Duration::new(10,0);
    // Wait for a result
    let result_string = wait_for_message(receiver, Some(topic.as_str()), wait_time, None)?;
    let result = result_string.to_uppercase() == "ENABLED";

    // Unsubscribe from state topic
    try_result!(client.unsubscribe(topic.as_str()), "Could not unsubscribe from controller state topic");

    return Ok(result);
}

fn start_device(config : &Configuration, mqtt_config: &MqttConfiguration) -> Result<bool, String> {
    fn start_helper(config: &Configuration, client: &mqtt::Client, receiver: &Receiver<Option<mqtt::Message>>, topic: &str, qos: i32) -> Result<bool,String> {
        let msg = mqtt::Message::new(topic, if config.start { "START_BOOT" } else { "START_RUN" }, qos);
        if client.publish(msg).is_err() {
            return Err("Could not send start initiation message".to_string());
        } else {
            println!("Sent initiation message to controller, waiting for reply...");
        }

        let timeout = Duration::new(config.timeout, 0);
        let received: String = wait_for_message(receiver, None, timeout, None)?;

        if received.to_lowercase().eq("disabled") {
            println!("Device '{}' is currently disabled for sync", config.device);
            return Ok(false);
        }

        // Device is already online
        if received.to_lowercase().eq("ready") {
            return Ok(true);
        }

        // Check only, do not boot, and device is offline
        if received.to_lowercase().eq("off") {
            return Ok(false);
        }

        // Wait until device is started
        if !(received.to_lowercase().eq("wait")) {
            return Err(format!("Expected to receive 'WAIT', but received '{}'", received));
        }

        // Second message should be CHECK
        let timeout2 = Duration::new(config.timeout, 0);
        let received2 = wait_for_message(receiver, None, timeout2, None)?;

        // Wait for check from controller to confirm still waiting
        if received2.to_lowercase().eq("check") {
            let msg = mqtt::Message::new(topic, "STILL_WAITING", qos);
            if client.publish(msg).is_err() {
                return Err(String::from("Could not send confirmation for still waiting"));
            }
        } else {
            return Err(format!("Expected to receive 'CHECK', but received '{}'", received2));
        }

        // Third message should just be confirmation with READY
        let timeout3 = Duration::new(config.timeout, 0);
        let received3 = wait_for_message(receiver, None, timeout3, None)?;

        // Return wether device is available or not
        return Ok(received3.to_lowercase().eq("ready"))
    };

    let qos = mqtt_config.qos;
    let topic_pub = get_topic_pub(&config, &mqtt_config);
    let (client,receiver) : (mqtt::Client,Receiver<Option<mqtt::Message>>) =
        try_result!(get_client(&config, &mqtt_config), "Could not create mqtt client and receiver");

    let is_controller_online = get_controller_state(&client, &receiver, &config, mqtt_config.qos)?;
    if !is_controller_online {
        println!("Controller for '{}' is not available", config.device.as_str());
        return Ok(false);
    }

    let result = start_helper(config, &client, &receiver, topic_pub.as_str(), qos);

    if client.disconnect(None).is_err() {
        println!("Error while disconnecting from mqtt broker");
    }

    return result;
}

fn stop_device(config : &Configuration, mqtt_config: &MqttConfiguration) -> Result<bool, String> {
    let qos = mqtt_config.qos;
    let topic_pub = get_topic_pub(&config, &mqtt_config);
    let (client,receiver) : (mqtt::Client,Receiver<Option<mqtt::Message>>) =
        try_result!(get_client(&config, &mqtt_config), "Could not create mqtt client and receiver");

    let is_controller_online = get_controller_state(&client, &receiver, &config, mqtt_config.qos)?;
    if !is_controller_online {
        println!("Controller for '{}' is not available", config.device.as_str());
        return Ok(false);
    }

    let msg = mqtt::Message::new(topic_pub, "DONE", qos);
    let result = if client.publish(msg).is_ok() {
        Ok(true)
    } else {
        Err("Could not send end message".to_string())
    };

    if client.disconnect(None).is_err() {
        println!("Error while disconnecting from mqtt broker");
    }

    return result;
}

fn get_client(config: &Configuration, mqtt_config: &MqttConfiguration) -> Result<(mqtt::Client,Receiver<Option<mqtt::Message>>), String> {
    let mqtt_host = format!("tcp://{}:{}", mqtt_config.host, mqtt_config.port);
    let mut client: mqtt::Client = try_result!(mqtt::Client::new(mqtt_host), "Failed connecting to broker");

    let mut options_builder = mqtt::ConnectOptionsBuilder::new();
    let mut options = options_builder.clean_session(true);
    options = options.user_name(&mqtt_config.user);
    if mqtt_config.password.is_some() {
        options = options.password(mqtt_config.password.as_ref().unwrap());
    }

    //options.connect_timeout()
    //options.automatic_reconnect()

    // Set last will in case of whatever failure that includes a interrupted connection
    let testament_topic = get_topic_pub(config, mqtt_config);
    let testament = mqtt::Message::new(testament_topic, "ABORT", mqtt_config.qos);
    options.will_message(testament);

    let topic_sub = get_topic_sub(&config, &mqtt_config);
    let qos = mqtt_config.qos;

    try_result!(client.connect(options.finalize()), "Could not connect to the mqtt broker");

    let receiver = client.start_consuming();
    try_result!(client.subscribe(&topic_sub, qos), "Could not subscribe to mqtt topic");

    Ok((client, receiver))
}

fn wait_for_message(receiver: &Receiver<Option<mqtt::Message>>, on_topic: Option<&str>, timeout: Duration, expected: Option<String>) -> Result<String, String> {
    let start_time = Instant::now();

    loop {
        let time_left = timeout - start_time.elapsed();

        let received : Option<mqtt::Message> = try_result!(receiver.recv_timeout(time_left), "Timeout exceeded");
        // TODO: What was this again?
        let received_message: mqtt::Message = try_option!(received, "Timeout on receive operation");
        // TODO: Reconnect on connection loss

        if let Some(expected_topic) = on_topic {
            if received_message.topic() != expected_topic {
                continue;
            }
        }

        let received_string = received_message.payload_str().to_string();
        if expected.is_some() {
            if received_string.eq(expected.as_ref().unwrap()) {
                return Ok(received_string);
            }
        } else {
            return Ok(received_string);
        }

        // Did not receive expected message -> Wait again
    }
}

fn get_topic_sub(config: &Configuration, mqtt_config: &MqttConfiguration) -> String {
    config.topic_sub.clone().unwrap_or("device/".to_string()
        .add(&config.device)
        .add("/controller/to/")
        .add(&mqtt_config.user))
}

fn get_topic_pub(config: &Configuration, mqtt_config: &MqttConfiguration) -> String {
    config.topic_pub.clone().unwrap_or("device/".to_string()
        .add(&config.device)
        .add("/controller/from/")
        .add(&mqtt_config.user))
}

fn get_controller_state_topic(config: &Configuration) -> String {
    return format!("device/{}/controller/status", config.device);
}