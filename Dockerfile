FROM node:24.21.0-bookworm-slim@sha256:d6aa754f16b3197301076f047b5def2f02ea1dbbc2ca920407d46d7ec7f87b20 AS web
WORKDIR /app
RUN npm install --global pnpm@12.10.1
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml .npmrc ./
COPY apps/web/package.json apps/web/package.json
RUN pnpm install --frozen-lockfile --ignore-scripts
COPY apps/web apps/web
ARG APP_ORIGIN=http://127.0.0.1:3000
ENV APP_ORIGIN=$APP_ORIGIN
RUN pnpm build

FROM rust:1.99.0-alpine3.24@sha256:0cce0a5e0e8ba67b455257a3a02a1d99005f382748789d6464460028810f1627 AS api
ARG TARGETARCH
WORKDIR /app
RUN apk add --no-cache build-base cmake perl linux-headers
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY services services
COPY db db
COPY scripts/media-sandbox.c /app/media-sandbox.c
RUN cc -std=c11 -O2 -Wall -Wextra -Werror -static -fstack-protector-strong /app/media-sandbox.c -o /app/myfans-media-sandbox
RUN --mount=type=cache,id=myfans-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=myfans-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=myfans-cargo-1.99-musl-${TARGETARCH},target=/app/target,sharing=locked \
    cargo build --release --locked -p myfans-api && cp target/release/myfans-api /app/myfans-api && cp target/release/myfans-media /app/myfans-media

FROM node:24.21.0-alpine3.24@sha256:ebfe2f90462722a7a4de65e91990e97fe0d401c70e0e762c5b53302f905ec1c1 AS video
WORKDIR /app
RUN apk add --no-cache bash ca-certificates curl build-base pkgconf nasm x264-dev xz
COPY scripts/install-ffmpeg.sh scripts/install-ffmpeg.sh
ENV FFMPEG_INSTALL_ROOT=/opt/ffmpeg
RUN bash scripts/install-ffmpeg.sh

FROM node:24.21.0-alpine3.24@sha256:ebfe2f90462722a7a4de65e91990e97fe0d401c70e0e762c5b53302f905ec1c1 AS runtime
ARG APP_ORIGIN=http://127.0.0.1:3000
LABEL io.myfans.web-origin=$APP_ORIGIN
LABEL io.myfans.runtime-distro="alpine:3.24"
RUN apk upgrade --no-cache && apk add --no-cache ca-certificates x264-libs \
    && rm -rf /usr/local/lib/node_modules/npm /usr/local/bin/npm /usr/local/bin/npx /usr/local/bin/yarn /usr/local/bin/yarnpkg /opt/yarn-*
WORKDIR /app
RUN printf '%s' "$APP_ORIGIN" > /app/web-origin
COPY --from=web --chown=node:node /app/apps/web/build apps/web/build
COPY --from=web --chown=node:node /app/apps/web/package.json apps/web/package.json
COPY --from=api /app/myfans-api /usr/local/bin/myfans-api
COPY --from=api /app/myfans-media /usr/local/bin/myfans-media
COPY --from=api /app/myfans-media-sandbox /usr/local/bin/myfans-media-sandbox
COPY --from=video /opt/ffmpeg/bin/ffmpeg /usr/local/bin/ffmpeg
COPY --from=video /opt/ffmpeg/bin/ffprobe /usr/local/bin/ffprobe
COPY --chown=node:node scripts/serve.mjs scripts/serve.mjs
COPY --chown=node:node scripts/healthcheck.mjs scripts/healthcheck.mjs
COPY --chown=node:node scripts/cloud-entrypoint.mjs scripts/cloud-worker-health.mjs scripts/
COPY --chown=node:node deploy/certs/global-bundle.pem certs/global-bundle.pem
COPY --chown=node:node THIRD_PARTY_NOTICES.md THIRD_PARTY_NOTICES.md
RUN mkdir -p /app/.local/media && chown -R node:node /app/.local
ENV BODY_SIZE_LIMIT=12M APP_MODE=sandbox HOST=0.0.0.0 PORT=3000 API_BINARY=/usr/local/bin/myfans-api API_BASE_URL=http://127.0.0.1:8080
USER node
EXPOSE 3000
HEALTHCHECK --interval=20s --timeout=5s --start-period=30s --retries=3 CMD ["node","scripts/healthcheck.mjs"]
CMD ["node","scripts/serve.mjs"]
