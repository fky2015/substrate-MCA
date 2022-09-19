#!/bin/bash

SLEEP=${SLEEP:-30}
LOG_LEVEL=${LOG_LEVEL:-afp=trace}

# wait for signal, then kill thread
trap 'pkill -P $$' SIGINT SIGTERM

RUST_LOG=$LOG_LEVEL ./target/debug/node-tendermint --alice --tmp --chain 4players --node-key 0000000000000000000000000000000000000000000000000000000000000001 --port 30334 2>&1 | tee alice.log &

RUST_LOG=$LOG_LEVEL ./target/debug/node-tendermint --bob --tmp --chain 4players --node-key 0000000000000000000000000000000000000000000000000000000000000002 --bootnodes /ip4/127.0.0.1/tcp/30334/p2p/12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp 2>&1 | tee bob.log &

RUST_LOG=$LOG_LEVEL ./target/debug/node-tendermint --charlie --tmp --chain 4players --node-key 0000000000000000000000000000000000000000000000000000000000000003 --bootnodes /ip4/127.0.0.1/tcp/30334/p2p/12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp 2>&1 | tee charlie.log &

sleep 36

RUST_LOG=$LOG_LEVEL ./target/debug/node-tendermint --dave --tmp --chain 4players --node-key 0000000000000000000000000000000000000000000000000000000000000004 --bootnodes /ip4/127.0.0.1/tcp/30334/p2p/12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp 2>&1 | tee dave.log &

# Exit after sleep 30 seconds.
sleep "$SLEEP";
pkill -P $$
