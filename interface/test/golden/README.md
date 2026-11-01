# Golden vectors

Each `<name>.txtpb` is a message in Protobuf text format, and `<name>.bin` is its wire encoding as produced by `protoc --encode`. They pin the wire format across the three implementations:

- `protoc` (the reference) produces the `.bin` files: `./regen.sh`
- C++ (`test/test_golden.cpp`): text → binary must equal the `.bin`, and the `.bin` must parse back to the same message.
- Rust (`controller/deck_msgs/tests/golden.rs`): decoding each `.bin` must give the expected field values, and re-encoding must give the same bytes.

Add a case by writing a `.txtpb` (first line `# message: remote_joy.v1.<Type>`), running `./regen.sh`, and adding it to both tests.
