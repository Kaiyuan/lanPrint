# End-to-end smoke tests

These scripts exercise the bridge WebSocket relay and the desktop cloud path.
They expect a bridge running on `127.0.0.1`:

```bash
# relay only
lanprint-bridge --addr 127.0.0.1:18081 --auth-code smoke-server-code \
  --data /tmp/lanprint-bridge-smoke.json
node scripts/e2e/bridge-relay.mjs

# desktop cloud host on 18082, then a client relay + authenticated API call
lanPrint --cloud http://127.0.0.1:18082 test-server-code
lanprint-bridge --addr 127.0.0.1:18082 --auth-code test-server-code \
  --data /tmp/lanprint-bridge-e2e.json
node scripts/e2e/cloud.mjs
node scripts/e2e/cloud-auth.mjs
```

The scripts use Node 22+ (global `fetch` and `WebSocket`). They are development
helpers and are not part of the release packages.
