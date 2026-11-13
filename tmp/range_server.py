import http.server, socketserver, os, sys, re, time, json
PORT = int(sys.argv[1]); PATH = sys.argv[2]; SIZE = int(sys.argv[3]); LOG = sys.argv[4]
if not os.path.exists(PATH) or os.path.getsize(PATH) != SIZE:
    with open(PATH, "wb") as f: f.truncate(SIZE)
T0 = time.time()
RATE = int(os.environ.get("DLM_RATE_KBPS", "0")) * 1024
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def do_GET(self):
        size = os.path.getsize(PATH)
        rng = self.headers.get("Range"); start, end, code = 0, size - 1, 200
        if rng:
            m = re.match(r"bytes=(\d*)-(\d*)", rng)
            if m:
                if m.group(1): start = int(m.group(1))
                if m.group(2): end = min(int(m.group(2)), size - 1)
                code = 206
        with open(LOG, "a", encoding="utf-8") as lg:
            lg.write(json.dumps({"t": round((time.time()-T0)*1000), "start": start, "end": end, "code": code}) + "\n")
        self.send_response(code)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        if code == 206: self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Content-Type", "application/octet-stream")
        self.end_headers()
        with open(PATH, "rb") as f:
            f.seek(start); left = end - start + 1; sent = 0; t0 = time.time()
            while left > 0:
                chunk = f.read(min(65536, left))
                if not chunk: break
                try: self.wfile.write(chunk)
                except Exception: return
                left -= len(chunk); sent += len(chunk)
                if RATE:
                    dt = sent / RATE - (time.time() - t0)
                    if dt > 0: time.sleep(dt)
socketserver.ThreadingTCPServer.allow_reuse_address = True
with socketserver.ThreadingTCPServer(("127.0.0.1", PORT), H) as s: s.serve_forever()