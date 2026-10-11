<script lang="ts">
	import { Badge } from "@/components/ui/svelte";
	import { desktopRemoteConnection } from "@/lib/remote-runtime";
</script>

{#if $desktopRemoteConnection}
	<Badge
		variant="outline"
		class="max-w-64 truncate gap-1.5"
		title={$desktopRemoteConnection.connected ? "Cluster access runs on this remote host" : "This remote host remains selected, but is disconnected"}
	>
		<span
			class={$desktopRemoteConnection.connected
				? "size-1.5 shrink-0 rounded-full bg-emerald-500"
				: "size-1.5 shrink-0 rounded-full bg-muted-foreground"}
			aria-hidden="true"
		></span>
		<span class="truncate">Remote host: {$desktopRemoteConnection.serverUrl}{ $desktopRemoteConnection.connected ? "" : " (disconnected)" }</span>
	</Badge>
{/if}
