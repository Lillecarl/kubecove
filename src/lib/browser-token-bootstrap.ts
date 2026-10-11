interface BrowserStartupTokenGlobal {
	__KUBECOVE_STARTUP_TOKEN__?: string;
}

if ("window" in globalThis) {
	const params = new URLSearchParams(window.location.hash.slice(1));
	const token = params.get("token");
	if (token) {
		// SAFETY: this module owns the temporary startup-token property and removes the URL fragment immediately.
		(window as Window & BrowserStartupTokenGlobal).__KUBECOVE_STARTUP_TOKEN__ = token;
		window.history.replaceState(window.history.state, "", `${window.location.pathname}${window.location.search}`);
	}
}
