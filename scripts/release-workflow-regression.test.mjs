import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const workflow = readFileSync(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");

function job(name) {
  const lines = workflow.split("\n");
  const start = lines.indexOf(`  ${name}:`);
  assert.notEqual(start, -1, `Missing job: ${name}`);
  let end = start + 1;
  while (end < lines.length && !/^ {2}[\w-]+:$/.test(lines[end])) end++;
  return lines.slice(start, end).join("\n");
}

test("both container variants are verified before registry publication", () => {
  const build = job("container-build");
  assert.match(build, /variant: \[full, slim\]/);
  assert.match(build, /docker build[^\n]+--target "\$VARIANT"/);
  assert.match(build, /scripts\/container-smoke\.sh/);
  assert.match(build, /docker save/);
  assert.doesNotMatch(build, /docker push|docker login/);
  const verify = job("verify");
  assert.match(verify, /- container-build/);
  assert.match(verify, /test "\$\{CONTAINER_BUILD_RESULT\}" = "success"/);
  const publish = job("publish");
  assert.match(publish, /packages: write/);
  assert.match(publish, /- container-build/);
  assert.ok(publish.indexOf("Publish container images") > publish.indexOf("Verify final updater platform coverage"));
  assert.match(publish, /docker load/);
  assert.match(publish, /--password-stdin/);
  assert.match(publish, /for variant in full slim/);
  assert.match(publish, /suffix="-slim"/);
});

test("headless release builds the three standalone targets from web assets", () => {
  const build = job("server-build");
  for (const target of [
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
  ]) assert.ok(build.includes(`target: ${target}`), target);

  assert.match(build, /name: Build embedded web assets\n {8}run: bun --bun vite build\n {8}env:\n {10}KUBECOVE_PUBLIC_RUNTIME: web/);
  assert.ok(build.indexOf("run: bun --bun vite build") < build.indexOf("run: cargo build --locked --release"));
  assert.match(build, /run: cargo build --locked --release --manifest-path src-tauri\/server\/Cargo\.toml --target \$\{\{ matrix\.target \}\}/);
  assert.match(build, /kubecove-server_\$\{version\}_\$\{TARGET\}\.tar\.gz/);
  assert.match(build, /actions\/upload-artifact@[a-f0-9]{40}/);
  assert.match(build, /sidecar="kubeconform-\$\{TARGET\}"/);
  assert.match(build, /test -x "src-tauri\/bin\/\$sidecar"/);
  assert.match(build, /tar -czf "\$archive"[^\n]+kubecove-server[^\n]+"\$sidecar"/);
  assert.doesNotMatch(build, /cache-apt-pkgs|apt-get install|webkit2gtk|appindicator|libsecret/);
});

test("server build reuses pinned actions already used by release jobs", () => {
  const build = job("server-build");
  const existingReleaseJob = job("frontend");
  for (const action of ["actions/checkout", "oven-sh/setup-bun", "actions/cache"]) {
    const pinned = existingReleaseJob.match(new RegExp(`uses: ${action.replaceAll("/", "\\/")}@([a-f0-9]{40})`));
    assert.ok(pinned, `Expected an existing pinned ${action} action`);
    assert.ok(build.includes(`uses: ${action}@${pinned[1]}`), `${action} pin differs`);
  }
  assert.match(build, /dtolnay\/rust-toolchain@[a-f0-9]{40}/);
  assert.match(build, /swatinem\/rust-cache@[a-f0-9]{40}/);
});

test("publishing can read build artifacts and waits for all release gates", () => {
  const publish = job("publish");
  assert.match(publish, /permissions:\n {6}actions: read\n {6}contents: write/);
  for (const need of ["prepare", "verify", "build", "server-build"]) {
    assert.match(publish, new RegExp(`- ${need}(?:\\n|$)`), need);
  }
  assert.match(publish, /gh run download/);
  assert.match(publish, /gh release upload[\s\S]*kubecove-server-/);
});

test("final asset verification requires each server archive before publishing", () => {
  const publish = job("publish");
  for (const asset of [
    /kubecove-server_\$\{version\}_x86_64-unknown-linux-gnu\.tar\.gz/,
    /kubecove-server_\$\{version\}_x86_64-apple-darwin\.tar\.gz/,
    /kubecove-server_\$\{version\}_aarch64-apple-darwin\.tar\.gz/,
  ]) assert.match(publish, asset);

  const verifyAt = publish.indexOf("Verify final updater platform coverage");
  const publishAt = publish.indexOf("Publish completed release");
  assert.notEqual(verifyAt, -1);
  assert.ok(publishAt > verifyAt, "release must remain unpublished until final verification completes");
  assert.match(publish.slice(publishAt), /if: needs\.prepare\.outputs\.release-draft == 'true'/);
});
