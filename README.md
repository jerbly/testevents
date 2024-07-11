# testevents

A service to make it easier to create spans in some restricted environments. Similar to [buildevents](https://github.com/honeycombio/buildevents/tree/main) but a simple RESTful server. This is useful when you can't execute a binary but need a way to open and close OpenTelemetry compatible spans. Low-code testing tools are a good use case.

**testevents** wraps the Honeycomb [Create Events API](https://docs.honeycomb.io/api/tag/Events#operation/createEvents) with an in-memory store, keyed on `trace_id` and `span_id`. When you open a span you provide a TTL. If you don't close it in time, say your script crashed, it will "close" the span with an error stating that the TTL ran out.

**testevents** uses the OpenTelemetry library to create the `trace_id` and `span_id` ensuring downstream compatibility. `/` and `/child/` also return a [`traceparent`](https://www.w3.org/TR/trace-context/#traceparent-header-field-values) that can be used in subsequent http calls for distributed tracing.

Spans are "closed" with `/close/{trace_id}/{span_id}/` - this creates the Honeycomb event with a calculated `duration_ms` from the "open" call: `/` or `/child/`.

## Installing

[Follow the instructions on the release page.](https://github.com/jerbly/testevents/releases) There are installers of pre-built binaries for popular OSes.

## Building

If you really want to build from source and not use a [pre-built binary release](https://github.com/jerbly/testevents/releases) then firstly you'll need a
[Rust installation](https://www.rust-lang.org/) to compile it:

```shell
$ git clone https://github.com/jerbly/testevents.git
$ cd testevents
$ cargo build --release
```

## Usage

### Environment variables / `.env` file entries

You must provide `HONEYCOMB_API_KEY`. This api key must have access to create datasets. An ingest key is ideal.

Provide `TESTEVENTS_PORT` to bind to an alternative from the default `3003`.

### Example

Request:
```shell
curl -i -X POST \
  'http://127.0.0.1:3003/' \
  -H 'Content-Type: application/json' \
  -d '{"service_name":"jerbly-test", 
       "name":"test", 
       "hello":"world", 
       "ttl":6000}'
```
Response:
```shell
HTTP/1.1 200 OK
content-type: application/json
content-length: 148
date: Thu, 11 Jul 2024 01:43:11 GMT

{"span_id":"c7bf50437dd159fe","trace_id":"b4a65a1ad17de44ce84a31df0504e5e3","traceparent":"00-b4a65a1ad17de44ce84a31df0504e5e3-c7bf50437dd159fe-01"}
```