FROM ekidd/rust-musl-builder as build

# create a dummy project to cache dependencies
RUN cargo init --name webhook-tester .
COPY --chown=rust:rust ./Cargo.toml ./Cargo.toml
COPY --chown=rust:rust ./Cargo.lock ./Cargo.lock
RUN cargo build --release
RUN rm -r src target/x86_64-unknown-linux-musl/release/deps/webhook_tester* target/x86_64-unknown-linux-musl/release/webhook-tester*

# copy the actual source and compile it
COPY --chown=rust:rust ./src ./src
COPY --chown=rust:rust ./pages ./pages
RUN cargo build --release

FROM scratch

COPY --from=build /home/rust/src/target/x86_64-unknown-linux-musl/release/webhook-tester /webhook-tester
CMD ["/webhook-tester"]
