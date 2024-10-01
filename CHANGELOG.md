# 0.4.0

- Switched to native OpenTelemetry for wide compatibility.
- `service.name` is now set in the `OTEL_SERVICE_NAME`.
- String, Boolean and Number attributes are supported.

# 0.3.0

- `service.name` must be sent vs `service_name` now for consistency
- a `PATCH` to update the `ttl` will extend from the current span duration. e.g. patching 10000 to a span that has been running for 34125ms will set the ttl to 44125
- added `test.sh` to run through the operations and expiration behaviour

# 0.2.1

- API changed to use HTTP verbs for crud

# 0.1.0

- First release
