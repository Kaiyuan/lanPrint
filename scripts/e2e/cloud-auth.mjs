const base = process.env.BRIDGE_URL || "http://127.0.0.1:18082";
const share = process.env.SHARE_CODE || "4YZTYYQF";
const session = await (await fetch(base + "/api/v1/clients/connect", {
  method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ share_code: share }),
})).json();
const ws = new WebSocket(`ws://127.0.0.1:18082/ws/client?token=${session.session_token}`);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
let nextId = 1;
const waiters = new Map();
ws.onmessage = (event) => {
  const frame = JSON.parse(event.data);
  if (frame.t === "response" || frame.t === "response_chunk") {
    const entry = waiters.get(frame.id);
    if (!entry) return;
    if (frame.t === "response") entry.status = frame.status;
    else {
      entry.chunks.push(Buffer.from(frame.data, "base64"));
      if (frame.last) { waiters.delete(frame.id); entry.resolve({ status: entry.status, body: Buffer.concat(entry.chunks).toString() }); }
    }
  }
};
function request(method, path, headers, body) {
  const id = nextId++;
  return new Promise((resolve) => {
    waiters.set(id, { chunks: [], resolve });
    ws.send(JSON.stringify({ t: "request", id, method, path, headers, content_length: body.length }));
    ws.send(JSON.stringify({ t: "request_chunk", id, data: body.toString("base64"), last: true }));
  });
}
const auth = await request("POST", "/api/v1/auth", [["content-type", "application/json"]], Buffer.from(JSON.stringify({ password: "test1234" })));
console.log("auth", auth.status, auth.body.slice(0, 60));
const token = JSON.parse(auth.body).token;
const printers = await request("GET", "/api/v1/printers", [["authorization", `Bearer ${token}`]], Buffer.alloc(0));
console.log("printers", printers.status, printers.body.slice(0, 120));
if (printers.status !== 200 || !printers.body.includes("KONICA")) process.exit(1);
ws.close();
console.log("cloud auth and printer list OK");
