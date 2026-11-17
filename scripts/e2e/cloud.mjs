const base = process.env.BRIDGE_URL || "http://127.0.0.1:18082";
const share = process.env.SHARE_CODE || "4YZTYYQF";
const response = await fetch(base + "/api/v1/clients/connect", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ share_code: share }),
});
const session = await response.json();
if (!response.ok) throw new Error(JSON.stringify(session));
const ws = new WebSocket(`ws://127.0.0.1:18082/ws/client?token=${session.session_token}`);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
const id = 12345;
const result = await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("timed out")), 10000);
  ws.onmessage = (event) => {
    const frame = JSON.parse(event.data);
    if (frame.t === "response_chunk" && frame.last) {
      clearTimeout(timer);
      resolve(Buffer.from(frame.data, "base64").toString());
    }
  };
  ws.send(JSON.stringify({ t: "request", id, method: "GET", path: "/api/v1/info", headers: [], content_length: 0 }));
  ws.send(JSON.stringify({ t: "request_chunk", id, data: "", last: true }));
});
console.log(result);
const info = JSON.parse(result);
if (info.device_name !== "KAIYUAN-AMD") process.exit(1);
ws.close();
console.log("cloud relay to desktop OK");
