FROM rust:1.67 AS build
WORKDIR /usr/src

# Download the target for static linking.
RUN rustup target add $(arch)-unknown-linux-musl

# Fix ring building using musl - see https://github.com/briansmith/ring/issues/1414#issuecomment-1055177218
RUN apt-get update && apt-get install musl-tools clang llvm -y
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-Clink-self-contained=yes -Clinker=rust-lld"

RUN mkdir /usr/src/rancher-project-info-propagator
WORKDIR /usr/src/rancher-project-info-propagator
COPY ./ ./
RUN cargo install --target x86_64-unknown-linux-musl --path .

FROM alpine AS cfg
RUN echo "controller:x:65533:65533::/tmp:/sbin/nologin" >> /etc/passwd
RUN echo "controller:x:65533:controller" >> /etc/group

# Copy the statically-linked binary into a scratch container.
FROM scratch
COPY --from=cfg /etc/passwd /etc/passwd
COPY --from=cfg /etc/group /etc/group
COPY --from=build --chmod=0755 /usr/local/cargo/bin/rancher-project-info-propagator /rancher-project-info-propagator
USER 65533:65533
CMD ["/rancher-project-info-propagator"]
