# Retain only signature data from the official image, never its OS or binaries.
FROM clamav/clamav:1.5.4-debian13-slim@sha256:9bb8712a50f0e75166e936c452cd82dd5e5be0b85586598930b5bbb84a99a578 AS signatures
FROM rust:1.99.0-alpine3.24@sha256:0cce0a5e0e8ba67b455257a3a02a1d99005f382748789d6464460028810f1627 AS build
RUN apk upgrade --no-cache && apk add --no-cache build-base cmake ninja curl ca-certificates git \
    clang-dev llvm-dev linux-headers bzip2-dev curl-dev json-c-dev libxml2-dev \
    ncurses-dev openssl-dev pcre2-dev zlib-dev libunwind-dev pkgconf
WORKDIR /src
RUN curl --fail --location --retry 3 --output clamav.tar.gz \
      https://github.com/Cisco-Talos/clamav/releases/download/clamav-1.5.4/clamav-1.5.4.tar.gz \
    && echo '1af1117a228f1b5bc7fa91a0dabc37848a99e7d25188e9be8043332ce721dfd3  clamav.tar.gz' | sha256sum -c \
    && tar -xzf clamav.tar.gz --strip-components=1 && rm clamav.tar.gz \
    && rm -rf .cargo/vendor .cargo/config.toml \
    && sed -i 's/set(CARGO_ARGS "build")/set(CARGO_ARGS "build" "--locked" "--offline")/' cmake/FindRust.cmake
# Compatible updates remove unsound-anyhow/rand advisories and yanked core2.
COPY deploy/aws/scanner.Cargo.lock /src/Cargo.lock
ENV RUSTFLAGS="-C target-feature=-crt-static" OPENSSL_NO_VENDOR=1
RUN cargo fetch --locked \
    && cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_INSTALL_PREFIX=/usr/local -DCMAKE_INSTALL_LIBDIR=lib \
      -DBYTECODE_RUNTIME=interpreter -DENABLE_CLAMONACC=OFF -DENABLE_MILTER=OFF \
      -DENABLE_MAN_PAGES=OFF -DENABLE_TESTS=OFF -DENABLE_DOXYGEN=OFF \
      -DENABLE_EXAMPLES=OFF -DENABLE_SYSTEMD=OFF -DENABLE_JSON_SHARED=ON \
      -DENABLE_STATIC_LIB=OFF -DENABLE_SHARED_LIB=ON \
    && cmake --build build --parallel 2 \
    && DESTDIR=/out cmake --install build \
    && mkdir -p /runtime/usr/local/bin /runtime/usr/local/sbin /runtime/usr/local/lib /runtime/usr/local/etc \
    && cp /out/usr/local/bin/freshclam /out/usr/local/bin/clamdscan /runtime/usr/local/bin/ \
    && cp /out/usr/local/sbin/clamd /runtime/usr/local/sbin/ \
    && cp -a /out/usr/local/lib/libclam*.so* /out/usr/local/lib/libfreshclam*.so* /runtime/usr/local/lib/ \
    && cp -a /out/usr/local/etc/certs /runtime/usr/local/etc/ \
    && mkdir -p /runtime/usr/share/licenses/clamav \
    && cp -a COPYING.txt COPYING /runtime/usr/share/licenses/clamav/

FROM alpine:3.24@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6
RUN apk upgrade --no-cache && apk add --no-cache ca-certificates libstdc++ libgcc \
      libcurl libxml2 pcre2 zlib libbz2 libunwind json-c \
    && addgroup -g 1000 clamav && adduser -D -H -u 1000 -G clamav clamav \
    && mkdir -p /opt/signatures /scan && chown 1000:1000 /scan
COPY --from=build /runtime/ /
COPY --from=signatures --chown=1000:1000 /var/lib/clamav/ /opt/signatures/
COPY --chmod=0644 deploy/aws/scanner.clamd.conf /etc/myfans-clamd.conf
COPY --chmod=0644 deploy/aws/scanner.freshclam.conf /etc/myfans-freshclam.conf
COPY --chmod=0755 deploy/aws/scanner-entrypoint.sh /usr/local/bin/scanner-entrypoint
COPY --chmod=0755 deploy/aws/scanner-health.sh /usr/local/bin/scanner-health
ENV TZ=UTC
RUN clamd --config-file=/etc/myfans-clamd.conf --version \
    && freshclam --config-file=/etc/myfans-freshclam.conf --version \
    && clamdscan --config-file=/etc/myfans-clamd.conf --version
USER 1000:1000
WORKDIR /scan
HEALTHCHECK --interval=30s --timeout=5s --start-period=300s --retries=3 CMD ["/usr/local/bin/scanner-health"]
ENTRYPOINT ["/usr/local/bin/scanner-entrypoint"]
