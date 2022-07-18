FROM rustlang/rust:nightly as builder

WORKDIR /build

RUN cargo install cargo-build-deps

RUN cargo new --bin gitalite
WORKDIR /build/gitalite

COPY Cargo.toml Cargo.lock ./
RUN cargo build-deps --release

COPY src/ src/

RUN cargo build --release

FROM pandoc/latex:latest-ubuntu

WORKDIR /app

COPY --from=builder /build/gitalite/target/release/gitalite ./

EXPOSE 3000
ENTRYPOINT ./gitalite --config $CONFIG
