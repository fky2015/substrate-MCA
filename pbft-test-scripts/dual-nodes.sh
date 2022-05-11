#!/bin/bash

SLEEP=${SLEEP:-30}
LOG_LEVEL=${RUST_LOG:-afp=info}

RUST_LOG=$LOG_LEVEL ./target/debug/node-template --alice --tmp --node-key 0000000000000000000000000000000000000000000000000000000000000001 --port 30334 2>&1 | tee alice.log &
THREAD_1=$!

RUST_LOG=$LOG_LEVEL ./target/debug/node-template --bob --tmp --bootnodes /ip4/127.0.0.1/tcp/30334/p2p/12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp 2>&1 | tee bob.log &
THREAD_2=$!

# wait for signal, then kill thread
trap 'kill $THREAD_1 $THREAD_2' SIGINT SIGTERM

# Exit after sleep 30 seconds.
sleep "$SLEEP";
pkill -P $$
