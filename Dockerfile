FROM rust:1.88.0-slim-bookworm AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
RUN cargo build --locked --release --package meshharbor --bin meshharbor

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 meshharbor \
    && useradd --uid 10001 --gid meshharbor --create-home meshharbor \
    && install -d -m 0700 -o meshharbor -g meshharbor /data

COPY --from=build /src/target/release/meshharbor /usr/local/bin/meshharbor
COPY --from=build /src/LICENSE /src/THIRD-PARTY-NOTICES.md /usr/share/doc/meshharbor/
COPY --from=build /src/apps/studio/THIRD-PARTY-NOTICES.md /src/apps/studio/THIRD-PARTY-LICENSES.txt /usr/share/doc/meshharbor/apps/studio/
COPY --from=build /src/runtimes/pixal3d/THIRD-PARTY-NOTICES.md /usr/share/doc/meshharbor/runtimes/pixal3d/
COPY --from=build /src/runtimes/pixal3d/THIRD-PARTY-LICENSES/ /usr/share/doc/meshharbor/runtimes/pixal3d/THIRD-PARTY-LICENSES/

ENV MESHHARBOR_HOME=/data
USER meshharbor:meshharbor
ENTRYPOINT ["/usr/local/bin/meshharbor"]
CMD ["serve", "--listen", "127.0.0.1:7432"]
