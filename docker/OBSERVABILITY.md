# Observability

## Grafana

### To see metrics

It is possible to see Celestia rollup metrics in grafana. Grafana and Prometheus are starting by default with Celestia nodes.

To let prometheus gather metrics from rollup binary, bind prometheus listener to all interfaces using `--prometheus-exporter-bind=0.0.0.0:9845` parameter.

```
cd ../examples/demo-rollup/ && cargo run -- \
   --da-layer celestia \
   --rollup-config-path demo_rollup_config.toml \
   --genesis-config-dir \
   ../test-data/genesis/demo/celestia \
   --prometheus-exporter-bind=0.0.0.0:9845
```

Grafana is available on `http://localhost:3000` with `admin:admin123` credentials.

### To save changes in dashboard

After you've done editing Dashboard, 
export it to JSON and save to a corresponding file in  [`dashboards`](../docker/grafana/dashboards) folder.
This way changes will be provisioned to the next start.
