#!/bin/sh
set -eu

mkdir -p /var/lib/skb
chown skb:skb /var/lib/skb
exec gosu skb skb-server "$@"
