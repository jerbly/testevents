#!/bin/bash
url="http://localhost:3003"

echo "Make a root span"
response=$(curl -s -X POST $url/ -H 'Content-Type: application/json' -d '{"service.name":"jerbly-test","name":"test","hello":"world","ttl":3600000}')
root_span_id=$(echo $response | jq -r '.span_id')
root_trace_id=$(echo $response | jq -r '.trace_id')
sleep 1

echo "Make a child span"
response=$(curl -s -X POST $url/$root_trace_id/$root_span_id/ -H 'Content-Type: application/json' -d '{"service.name":"jerbly-test","name":"child","ttl":3600000}')
span_id=$(echo $response | jq -r '.span_id')
sleep 1

echo "Patch the child span with a new value and a shorter ttl - and let it timeout"
curl -s -X PATCH $url/$root_trace_id/$span_id/ -H 'Content-Type: application/json' -d '{"new":true,"ttl":1000}'
sleep 2

echo ""
echo "Check for a 404 on an attempt to DELETE the expired child span"
response=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE $url/$root_trace_id/$span_id/)
echo "Status code: $response"

echo "Close the root span"
curl -s -X DELETE $url/$root_trace_id/$root_span_id/
