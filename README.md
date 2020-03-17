# MQTT device controller client

## What does this do?
This interacts with the [MQTT device controller](https://github.com/lunarys/mqtt-device-controller) in order to remotely start and stop devices. It is designed to work autonomously in scripts.

## Usage

`./mqtt-controller-client <begin|end> [options]`

Options:
| Name | Default | Required | Description |
| -d, --device | | yes | The name of the device to start. Used in the default MQTT topics. |
| -h, --host | | yes | The host of the MQTT broker. |
| -p, --port | 1883 | no | The port of the MQTT broker. |
| -q, --qos | 1 | no | The quality of service for the MQTT broker. |
| -u, --user | | yes | The user for the MQTT broker. |
| -P, --password | | no | The password to connect to the MQTT broker. |
| -t, --timeout | 600 | no | Time to wait for a message from the controller in seconds. Useful if the device does not start for some reason. |
| --passive, --no-start | false | no | Do not actually start the device, but register for using it if it is online. |
| --topic-sub | `device/$DEVICE/controller/to/$USER` | no | Topic used for receiving messages from the controller. |
| --topic-pub | `device/$DEVICE/controller/from/$USER` | no | Topic used for sending messages to the controller. |
