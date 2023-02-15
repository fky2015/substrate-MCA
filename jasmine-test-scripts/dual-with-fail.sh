#!/bin/bash

SLEEP=${SLEEP:-30}
LOG_LEVEL=${LOG_LEVEL:-afp=trace}

RUST_LOG=$LOG_LEVEL ./target/debug/node-jasmine --alice --tmp --node-key 0000000000000000000000000000000000000000000000000000000000000001 --port 30334 2>&1 | tee alice.log &
THREAD_1=$!

# let alice enter view_change mode.
sleep 10

RUST_LOG=$LOG_LEVEL ./target/debug/node-jasmine --bob --tmp --bootnodes /ip4/127.0.0.1/tcp/30334/p2p/12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp 2>&1 | tee bob.log &
THREAD_2=$!

# wait for signal, then kill thread
trap 'kill $THREAD_1 $THREAD_2' SIGINT SIGTERM

# Exit after sleep 30 seconds.
sleep "$SLEEP";
pkill -P $$
