export const IS_DEV_BUILD = process.env.KUBECOVE_PUBLIC_DEV === "true";

export const IS_WEB_RUNTIME = process.env.KUBECOVE_PUBLIC_RUNTIME === "web";

export const RELEASE_CHANNEL =
	process.env.KUBECOVE_PUBLIC_RELEASE_CHANNEL === "stable" ? "stable" : "dev";
