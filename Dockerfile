# syntax=docker/dockerfile:1
FROM oven/bun:1.4.0-slim@sha256:e0ee68d16ccb9927bf02aa7dd8fd4bf3369ee6d46da04faa72b05ce8bfd135f6 AS frontend
WORKDIR /source
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY . .
RUN KUBECOVE_PUBLIC_RUNTIME=web bun run build

FROM rust:1.99.0-slim-trixie@sha256:2752b332db73fdbb7dc576f06c82ed1f312005784ef913d7e04a28f5f55dc581 AS backend
ARG TARGETARCH
WORKDIR /source
COPY src-tauri/ src-tauri/
# The bundled Linux kubeconform sidecar is currently x64 only.
RUN test "$TARGETARCH" = amd64
ENV CARGO_PROFILE_RELEASE_STRIP=debuginfo

FROM backend AS backend-slim
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/source/src-tauri/server/target,sharing=locked \
    cargo build --release --locked --no-default-features --manifest-path src-tauri/server/Cargo.toml \
    && cp src-tauri/server/target/release/kubecove-server /kubecove-server

FROM backend AS backend-full
COPY --from=frontend /source/dist/ dist/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/source/src-tauri/server/target,sharing=locked \
    cargo build --release --locked --manifest-path src-tauri/server/Cargo.toml \
    && cp src-tauri/server/target/release/kubecove-server /kubecove-server

FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f AS runtime
ARG VERSION
ARG REVISION
LABEL org.opencontainers.image.source="https://github.com/Timpan4/kubecove" \
      org.opencontainers.image.version="$VERSION" \
      org.opencontainers.image.revision="$REVISION" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -r /var/lib/apt/lists/* \
    && install -d -m 0700 -o 65534 -g 65534 /data /data/kubecove
COPY --chmod=0755 src-tauri/bin/kubeconform-x86_64-unknown-linux-gnu /usr/local/bin/kubeconform
COPY LICENSE /usr/share/doc/kubecove/LICENSE
# Debian's existing nobody user and nogroup group.
USER 65534:65534
EXPOSE 8080
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/kubecove-server"]
CMD ["--listen", "127.0.0.1:8080", "--data-dir", "/data/kubecove"]

FROM runtime AS slim
COPY --from=backend-slim --chmod=0755 /kubecove-server /usr/local/bin/kubecove-server

# Keep the browser UI image as the default build target.
FROM runtime AS full
COPY --from=backend-full --chmod=0755 /kubecove-server /usr/local/bin/kubecove-server
