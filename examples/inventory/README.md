# Inventory dashboard

This Rinzu application serves a dashboard, a JSON catalog, and a token-protected API for product details, stock, and quotes. The web process reads the catalog and three browser assets at startup. On the stock route it sends an authenticated JSON POST to a second Rinzu process and validates the response status, content type, and JSON data.

## Run the two services

Run both commands from this example directory using a current Rinzu CLI and its bundled SDK. Generate a token and start the warehouse in one terminal.

```sh
export INVENTORY_TOKEN="$(openssl rand -hex 24)"
printf '%s\n' "$INVENTORY_TOKEN"
INVENTORY_ROLE=warehouse rinzu run .
```

Start the web service in a second terminal with the same token.

```sh
export INVENTORY_TOKEN='token printed by the first terminal'
rinzu run .
```

Open `http://127.0.0.1:8080`, enter the token, select a product, and request a quote. The warehouse defaults to port 8081. `INVENTORY_PORT` overrides the local listener port and `INVENTORY_WAREHOUSE_PORT` selects the upstream port for the web process. `INVENTORY_ROOT` selects the directory containing `data/` and `public/`. SIGINT and SIGTERM request a graceful shutdown.

| Web route | Behavior |
| --- | --- |
| `GET /`, `GET /assets/inventory.css`, `GET /assets/inventory.js` | Serve HTML, CSS, and JavaScript with MIME and cache headers |
| `GET /api/catalog?limit=8` | List catalog items, with a validated limit from 1 through 16 |
| `GET /api/items/:sku` | Return a product description and its unit price in USD cents |
| `GET /api/items/:sku/stock` | Read available stock and warehouse location over HTTP |
| `POST /api/items/:sku/quote` | Accept JSON such as `{"quantity":2}` and calculate the total in USD cents |

The `/api/items` group and the warehouse's `POST /stock` require `Authorization: Bearer <token>`. The shared token must contain 20–128 URL-safe letters, digits, hyphens, or underscores. Both listeners bind to IPv4 loopback. The warehouse accepts `Content-Type: application/json` and a body such as `{"sku":"cable-1m"}`.

## Where the SDK fits

- The application registers sync and async handlers, groups routes, installs a synchronous authorization middleware and an API error policy, and builds typed HTTP responses. The stock handler uses `rinzu.json` and a pool of 16 single-operation `rinzu.http` clients. `PreparedHeaders` encodes the shared Authorization and Content-Type fields once, and its `send` method returns the response headers. [Application source](src/main.rinzu)
- Both processes use `rinzu.http.server.Server.listen` and `serveUntil` with default connection, timeout, and shutdown settings. Application code only registers signals, selects the bind address and port, and prints the listening address. [HTTP listener](../../sdk/http/README.md#http11-listener)
