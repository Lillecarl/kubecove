FROM oven/bun:1-slim AS frontend
WORKDIR /source
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY . .
RUN KUBECOVE_PUBLIC_RUNTIME=web bun run build

FROM rust:1-slim-trixie AS backend
ARG TARGETARCH
WORKDIR /source
COPY src-tauri/ src-tauri/
COPY --from=frontend /source/dist/ dist/
# The bundled Linux kubeconform sidecar is currently x64 only.
RUN test "$TARGETARCH" = amd64 \
    && cargo build --release --locked --manifest-path src-tauri/server/Cargo.toml

FROM debian:trixie-slim
LABEL org.opencontainers.image.source="https://github.com/Timpan4/kubecove" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -r /var/lib/apt/lists/* \
    && install -d -m 0700 -o 65534 -g 65534 /data /data/kubecove
COPY --from=backend --chmod=0755 /source/src-tauri/server/target/release/kubecove-server /usr/local/bin/kubecove-server
COPY --from=backend --chmod=0755 /source/src-tauri/bin/kubeconform-x86_64-unknown-linux-gnu /usr/local/bin/kubeconform
COPY LICENSE /usr/share/doc/kubecove/LICENSE
# Debian's existing nobody user and nogroup group.
USER 65534:65534
EXPOSE 8080
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/kubecove-server"]
CMD ["--listen", "127.0.0.1:8080", "--data-dir", "/data/kubecove"]
