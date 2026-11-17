// Quick end-to-end relay smoke test for the lanPrint bridge.
const base = "http://127.0.0.1:18081";
const auth = "smoke-server-code";

async function json(path, options = {}) {
  const response = await fetch(base + path, options);
  const text = await response.text();
  if (!response.ok) throw new Error(`${path}: ${response.status} ${text}`);
  return JSON.parse(text);
}

const reg = await json("/api/v1/hosts/register", {
  method: "POST",
  headers: { authorization: `Bearer ${auth}`, "content-type": "application/json" },
  body: JSON.stringify({ device_id: "smoke-host", device_name: "Smoke", version: "2", api_port: 52333, old_share_code: "" }),
});
console.log("registered", reg.share_code);

const host = new WebSocket(`ws://127.0.0.1:18081/ws/host?token=${reg.host_token}`);
await new Promise((resolve, reject) => { host.onopen = resolve; host.onerror = reject; });
host.send(JSON.stringify({ t: "hello", role: "host", device_id: "smoke-host", name: "Smoke" }));

host.onmessage = (event) => {
  const frame = JSON.parse(event.data);
  if (frame.t === "request") {
    host.send(JSON.stringify({ t: "response", id: frame.id, status: 200, headers: [["content-type", "application/json"]], content_length: 11 }));
    host.send(JSON.stringify({ t: "response_chunk", id: frame.id, data: Buffer.from('{"ok":true}').toString("base64"), last: true }));
  }
};

const session = await json("/api/v1/clients/connect", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ share_code: reg.share_code }),
});
const client = new WebSocket(`ws://127.0.0.1:18081/ws/client?token=${session.session_token}`);
await new Promise((resolve, reject) => { client.onopen = resolve; client.onerror = reject; });

const result = await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("relay timed out")), 5000);
  client.onmessage = (event) => {
    const frame = JSON.parse(event.data);
    if (frame.t === "response_chunk" && frame.last) {
      clearTimeout(timer);
      resolve(Buffer.from(frame.data, "base64").toString());
    }
  };
  client.send(JSON.stringify({ t: "request", id: 42, method: "GET", path: "/api/v1/info", headers: [], content_length: 0 }));
  client.send(JSON.stringify({ t: "request_chunk", id: 42, data: "", last: true }));
});

console.log("relay response", result);
if (!result.includes('"ok":true')) process.exit(1);
host.close();
client.close();
console.log("bridge relay OK");
