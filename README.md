# AudioZones

This is a set of two apps to manage and configure PipeWire in Linux environment. 

## Pipewire Web Server

An API Server app designed to run on Linux that effectively works as a PipeWire configuration server - it's just API endpoints to add/remove/list/configure pipewire connections.   The most often used use case would be to attach some source of audio, likely a USB device with audio input, to the default audio output or outputs on the server.   In the test setup, we have an eight-channel USB audio card and a two-channel USB input device ( Google cast capable )

## Android App

Sample Android app that uses Flutter and connects back to the pipe wire configuration server in order to configure various zones on the server I.e., change volume levels or turn them on or off 

