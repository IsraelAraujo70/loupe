# The Loupe website: build.py renders public/, Caddy serves it.
FROM python:3.12.7-alpine AS build
WORKDIR /site
COPY . .
RUN python3 build.py

FROM caddy:2.8.4-alpine
COPY Caddyfile /etc/caddy/Caddyfile
COPY --from=build /site/public /srv
