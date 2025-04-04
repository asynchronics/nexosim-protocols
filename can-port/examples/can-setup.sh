#!/bin/bash

if [ "$EUID" -ne 0 ]
then
    echo "Please run as root." >&2
    exit 1
fi

VCAN_LOADED=$(lsmod | grep ^vcan)
if [ -z "${VCAN_LOADED}" ]; then
    if ! modprobe vcan ; then
        echo "Unable to load the 'vcan' kernel module." >&2
        exit 1
    fi
fi

ip link add dev vcan0 type vcan
ip link set up vcan0
ip link add dev vcan1 type vcan
ip link set up vcan1
