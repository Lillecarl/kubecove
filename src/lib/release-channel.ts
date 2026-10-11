import { IS_WEB_RUNTIME, RELEASE_CHANNEL } from "./build-env";

export type AppReleaseChannel = "dev" | "stable";

export const APP_RELEASE_CHANNEL: AppReleaseChannel =
	RELEASE_CHANNEL;

let appUpdatesEnabledForTests: boolean | null = null;

export function isAppUpdatesEnabled(): boolean {
	return !IS_WEB_RUNTIME && (appUpdatesEnabledForTests ?? APP_RELEASE_CHANNEL === "stable");
}

export function setAppUpdatesEnabledForTests(enabled: boolean | null): void {
	appUpdatesEnabledForTests = enabled;
}
