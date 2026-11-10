"""Call one Kantor MCP tool: mcp_call.py <tool> '<json args>'. Uses KANTOR_SOCKET_DIR and KANTOR_BIN from env."""
import json, os, subprocess, sys

tool, args = sys.argv[1], json.loads(sys.argv[2])
p = subprocess.Popen([os.environ.get("KANTOR_BIN", "kantor"), "mcp", "--no-launch"],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def send(m):
    p.stdin.write(json.dumps(m) + "\n"); p.stdin.flush()
send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
    "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "ksite", "version": "1"}}})
print(p.stdout.readline().strip()[:200])
send({"jsonrpc": "2.0", "method": "notifications/initialized"})
send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": tool, "arguments": args}})
print(p.stdout.readline().strip()[:400])
p.stdin.close(); p.wait(timeout=10)
