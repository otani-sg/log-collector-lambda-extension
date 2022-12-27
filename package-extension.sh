#!/bin/bash -x
cargo build --release
mkdir -p ./target/extensions
cp ./target/release/log_collector_lambda_extension ./target/extensions
cd ./target
rm -f log_collector_lambda_extension.zip
zip log_collector_lambda_extension.zip extensions/log_collector_lambda_extension