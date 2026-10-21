#!/bin/sh
udevadm control --reload-rules && udevadm trigger --subsystem-match=hidraw || true
exit 0
