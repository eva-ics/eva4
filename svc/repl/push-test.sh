#!/bin/sh -xe

sudo lxc exec rtest1 eva server stop
sudo lxc file push /opt/eva4/target/debug/eva-repl rtest1/opt/eva4/svc/
sudo lxc exec rtest1 eva server start
