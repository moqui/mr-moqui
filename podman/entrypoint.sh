#!/bin/sh
# Prepare the data volume, then run the moqui binary as uid 1000.
# Embedded Postgres refuses to run as root. This script starts as root only so it can
# chown a fresh volume, then setpriv drops to the moqui user before the binary.
set -eu

data=/var/lib/moqui
runtime=/opt/moqui

link() {
    name=$1
    target=$2
    dest=$runtime/$name
    if [ -L "$dest" ] || [ ! -e "$dest" ]; then
        ln -sfn "$target" "$dest"
    fi
}

if [ "$(id -u)" = 0 ]; then
    install -d -o moqui -g moqui \
        "$data" "$data/db" "$data/log" "$data/cert" "$data/tmp" "$data/install" "$data/txlog"
    if [ ! -e "$data/env" ]; then
        install -o moqui -g moqui -m 600 /dev/null "$data/env"
    else
        chown moqui:moqui "$data/env"
    fi
    # the runtime directory stays as the image made it (root's, group moqui, sticky), so the binary and the conf in it
    # are not the moqui user's to replace (SEC-OUT-13)
    chown moqui:moqui "$data"
    link db "$data/db"
    link log "$data/log"
    link cert "$data/cert"
    link tmp "$data/tmp"
    link install "$data/install"
    link txlog "$data/txlog"
    link .env "$data/env"
    exec setpriv --reuid=moqui --regid=moqui --init-groups --inh-caps=-all /opt/moqui/moqui "$@"
fi

exec /opt/moqui/moqui "$@"
