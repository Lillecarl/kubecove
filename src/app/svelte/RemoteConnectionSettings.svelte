<script lang="ts">
	import { onMount } from "svelte";
	import { Button, Input, Label } from "@/components/ui/svelte";
	import { messageFromError } from "@/lib/error-redaction";
	import {
		connectDesktopRemote,
		desktopRemoteConnection,
		disconnectDesktopRemote,
		initializeDesktopRemote,
	} from "@/lib/remote-runtime";

	let serverUrl = $state("");
	let token = $state("");
	let pending = $state(false);
	let error = $state<string | null>(null);

	$effect(() => {
		if ($desktopRemoteConnection) serverUrl = $desktopRemoteConnection.serverUrl;
	});

	onMount(() => {
		void initializeDesktopRemote().catch((reason) => {
			error = messageFromError(reason);
		});
	});

	async function connect() {
		pending = true;
		error = null;
		try {
			await connectDesktopRemote(serverUrl, token);
			token = "";
		} catch (reason) {
			error = messageFromError(reason);
		} finally {
			pending = false;
		}
	}

	async function disconnect() {
		pending = true;
		error = null;
		try {
			await disconnectDesktopRemote();
			token = "";
		} catch (reason) {
			error = messageFromError(reason);
		} finally {
			pending = false;
		}
	}
</script>

{#if $desktopRemoteConnection?.connected}
	<div class="flex flex-col gap-3 text-sm">
		<p>Connected to <span class="font-medium">{$desktopRemoteConnection.serverUrl}</span>.</p>
		<p class="text-muted-foreground">Kubernetes requests and port forwards run through this server. Forwarded ports listen only on 127.0.0.1 on this laptop.</p>
		<p class="text-muted-foreground">Server {$desktopRemoteConnection.serverVersion}. Session grace: {$desktopRemoteConnection.sessionGraceSeconds} seconds.</p>
		<div><Button type="button" variant="destructive" disabled={pending} onclick={disconnect}>{pending ? "Disconnecting…" : "Disconnect"}</Button></div>
	</div>
{:else}
	{#if $desktopRemoteConnection}
		<div class="flex flex-wrap items-center justify-between gap-3 text-sm">
			<p class="text-muted-foreground">{$desktopRemoteConnection.serverUrl} is selected but disconnected. Cluster commands will not fall back to this laptop.</p>
			<Button type="button" variant="destructive" disabled={pending} onclick={disconnect}>{pending ? "Disconnecting…" : "Disconnect"}</Button>
		</div>
	{/if}
	<form class="flex max-w-xl flex-col gap-3" onsubmit={(event) => { event.preventDefault(); void connect(); }}>
		<div class="flex flex-col gap-2">
			<Label for="remote-server-url">Server URL</Label>
			<Input id="remote-server-url" type="url" autocomplete="url" placeholder="https://kube.example.com" bind:value={serverUrl} required />
		</div>
		<div class="flex flex-col gap-2">
			<Label for="remote-server-token">Server token</Label>
			<Input id="remote-server-token" type="password" autocomplete="off" bind:value={token} required />
			<p class="text-xs text-muted-foreground">The token stays in native Rust memory until you disconnect or close the app.</p>
		</div>
		{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
		<div><Button type="submit" disabled={pending || !serverUrl.trim() || !token.trim()}>{pending ? "Connecting…" : "Connect"}</Button></div>
	</form>
{/if}
{#if $desktopRemoteConnection?.connected && error}<p role="alert" class="mt-3 text-sm text-destructive">{error}</p>{/if}
