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
            .add_argument("operation", Store, "Operation to perform")
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
            .add_option(&["-p", "--passive", "--no-start"], StoreTrue, "Only check if the device is available, do not start it");
        parser.refer(&mut config.topic_sub)
            .add_option(&["--topic-sub"], StoreOption, "Topic used for subscribing to messages from the controller (default=device/$device/save/$user/status)");
        parser.refer(&mut config.topic_pub)
            .add_option(&["--topic-pub"], StoreOption, "Topic used for publishing messages to the controller (default=device/$device/save/$user/status/desired)");
        parser.refer(&mut mqtt_config.qos)
            .add_option(&["-q", "--qos"], Store, "Quallity of service for the communication with the mqtt broker (default=1)");
        parser.parse_args_or_exit();
    }

    let result = match operation.as_str() {
        "begin" => start_backup(&config, &mqtt_config),
        "end" => end_backup(&config, &mqtt_config),
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

fn start_backup(config : &Configuration, mqtt_config: &MqttConfiguration) -> Result<bool, String> {
    let qos = mqtt_config.qos;
    let topic_pub = get_topic_pub(&config, &mqtt_config);
    let (client,receiver) : (mqtt::Client,Receiver<Option<mqtt::Message>>) =
        try_result!(get_client(&config, &mqtt_config), "Could not create mqtt client and receiver");

    let msg = mqtt::Message::new(topic_pub, if config.start { "START_BOOT" } else { "START_RUN" }, qos);
    if client.publish(msg).is_err() {
        return Err("Could not send start initiation message".to_string());
    } else {
        println!("Sent initiation message to controller, waiting for reply...");
    }

    let timeout = Duration::new(config.timeout, 0);
    let received: String = wait_for_message(&receiver, timeout, None)?;

    let result = match received.to_lowercase().as_str() {
        "ready" => true,
        "disabled" => {
            println!("Device '{}' is currently disabled for sync", config.device);
            false
        },
        _ => false
    };

    if client.disconnect(None).is_err() {
        println!("Error while disconnecting from mqtt broker");
    }

    return Ok(result);
}

fn end_backup(config : &Configuration, mqtt_config: &MqttConfiguration) -> Result<bool, String> {
    let qos = mqtt_config.qos;
    let topic_pub = get_topic_pub(&config, &mqtt_config);
    let (client,_) : (mqtt::Client,Receiver<Option<mqtt::Message>>) =
        try_result!(get_client(&config, &mqtt_config), "Could not create mqtt client and receiver");

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
    //options.will_message()

    let topic_sub = get_topic_sub(&config, &mqtt_config);
    let qos = mqtt_config.qos;

    try_result!(client.connect(options.finalize()), "Could not connect to the mqtt broker");

    let receiver = client.start_consuming();
    try_result!(client.subscribe(&topic_sub, qos), "Could not subscribe to mqtt topic");

    Ok((client, receiver))
}

fn wait_for_message(receiver: &Receiver<Option<mqtt::Message>>, timeout: Duration, expected: Option<String>) -> Result<String, String> {
    let start_time = Instant::now();

    loop {
        let time_left = timeout - start_time.elapsed();

        let received : Option<mqtt::Message> = try_result!(receiver.recv_timeout(time_left), "Timeout exceeded");
        // TODO: What was this again?
        let received_message: mqtt::Message = try_option!(received, "Timeout on receive operation");
        // TODO: Reconnect on connection loss

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
        .add("/save/")
        .add(&mqtt_config.user)
        .add("/status"))
}

fn get_topic_pub(config: &Configuration, mqtt_config: &MqttConfiguration) -> String {
    config.topic_pub.clone().unwrap_or("device/".to_string()
        .add(&config.device)
        .add("/save/")
        .add(&mqtt_config.user)
        .add("/status/desired"))
}