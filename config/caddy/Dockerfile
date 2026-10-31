# Caddy with the deSEC DNS module, for certificates on a domain of your own (HOST_TLS=desec).
FROM caddy:2.11.7-builder AS build
RUN xcaddy build --with github.com/caddy-dns/desec
FROM caddy:2.11.7
COPY --from=build /usr/bin/caddy /usr/bin/caddy
