#!/bin/bash
which cargo > /tmp/cargo_path.txt 2>&1
cargo --version >> /tmp/cargo_path.txt 2>&1
cat /tmp/cargo_path.txt
