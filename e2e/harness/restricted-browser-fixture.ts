import { join, resolve } from "node:path";
import { createDevMockTauriClient } from "../../src/lib/tauri-dev-mocks";
import type { ResourceListRequest, ResourceScopeResult } from "../../src/lib/types";

// UI fixture only. headless-smoke.ts exercises the real backend's 403 handling.
const root = resolve(import.meta.dir, "../..");
const client = createDevMockTauriClient();
const server = Bun.serve({
	hostname: "127.0.0.1", port: 0,
	async fetch(request, server) {
		const url = new URL(request.url);
		if (url.pathname === "/api/v1/auth") return Response.json({ mode: "cloudflareAccess" });
		if (url.pathname === "/api/v1/session") return Response.json({ resumeToken: "ui-fixture", sessionGraceSeconds: 300, serverVersion: "fixture", credentialStore: "encryptedFile" });
		if (url.pathname === "/api/v1/channels" && server.upgrade(request)) return;
		if (url.pathname.startsWith("/api/v1/invoke/")) {
			const command = decodeURIComponent(url.pathname.split("/").at(-1) ?? "");
			const args = await request.json();
			if (command === "list_resource_scope") {
				const requests: ResourceListRequest[] = args.requests;
				const denied = requests.filter((item) => item.kind === "Secret");
				const allowed = requests.filter((item) => item.kind !== "Secret");
				const result = await client.invoke<ResourceScopeResult>(command, { ...args, requests: allowed });
				return Response.json({ ...result, failures: denied.map((item) => ({ clusterContext: args.clusterContext, request: item, error: { kind: "forbidden", message: "Secrets are forbidden for the fixture account" } })) });
			}
			try { return Response.json(await client.invoke(command, args)); }
			catch { return Response.json({ kind: "unsupportedOperation", message: "Not supported by the UI fixture" }, { status: 422 }); }
		}
		if (url.pathname.startsWith("/api/")) return Response.json(true);
		const asset = Bun.file(join(root, "dist", url.pathname === "/" ? "index.html" : url.pathname));
		if (!(await asset.exists())) return new Response("Not found", { status: 404 });
		if (url.pathname === "/") {
			const html = (await asset.text()).replace(/<meta\s+http-equiv="Content-Security-Policy"[^>]*>/i, "");
			return new Response(html, { headers: { "Content-Type": "text/html", "Content-Security-Policy": `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data: blob:; connect-src 'self' ws://${url.host}` } });
		}
		return new Response(asset);
	},
	websocket: { message() {} },
});
console.log(server.url.toString());
for (const signal of ["SIGINT", "SIGTERM"] as const) process.once(signal, () => server.stop(true));
