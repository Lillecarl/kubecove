# Headless container deployment

The root `Dockerfile` builds `kubecove-server`, its embedded browser app and the bundled kubeconform sidecar. The runtime image runs as Debian's unprivileged `nobody` user, UID/GID `65534`, and contains no desktop runtime. The image target is `linux/amd64`, matching the bundled Linux sidecar. No prebuilt image has been published, so build it yourself.

For client behaviour, authentication modes and server options, see [Run KubeCove on a remote host](../docs/wiki/Remote-Server.md).

## Build the image

From the repository root:

```sh
docker build --pull --platform linux/amd64 -t kubecove-server:local .
docker run --rm kubecove-server:local --help
```

The build uses the official [Bun image](https://bun.com/docs/installation#docker), [Rust image](https://hub.docker.com/_/rust) and [Debian stable image](https://hub.docker.com/_/debian). Frontend and Rust dependencies use the committed lockfiles. `.dockerignore` limits the build context to the required inputs.

The image's default command listens on container loopback, so Docker port publishing alone cannot reach it. On a standalone Docker host, either keep loopback and add a tunnel container in the same network namespace, or listen on `0.0.0.0:8443` with `--allow-remote`, `--tls-cert`, `--tls-key` and an exact HTTPS `--public-origin`. Mount certificate, key and kubeconfig files read-only. Persist `/data`; the server creates its private `0700` directory at `/data/kubecove`.

In token mode, startup stdout contains the login URL and token. Discard stdout when running the image, as the Kubernetes example does, and read `/data/kubecove/server-token` privately instead. Otherwise, treat persistent `docker logs` output as a credential.

## Choose Kubernetes permissions

The server acts in the cluster with one identity: the kubeconfig credentials you mount. Every user who can sign in to KubeCove shares that identity, whichever authentication mode you choose. OIDC and Cloudflare Access decide who may sign in. They do not pass user identities to Kubernetes RBAC.

The example binds a dedicated ServiceAccount to the built-in `view` ClusterRole across namespaces. `view` excludes Secrets, Pod exec, port forwarding and resource changes, so some panels show permission errors. KubeCove reports those resource types as unavailable and keeps showing the readable ones.

Broader permissions are an operator decision about what every permitted user may do. Grant the smallest role that covers the workflows you need. Binding `cluster-admin` gives every signed-in user full cluster control through KubeCove's guarded workflows. Do not bind it only to remove permission errors. See [Kubernetes RBAC](https://kubernetes.io/docs/reference/access-authn-authz/rbac/).

## Run inside k0s

The [Kubernetes example](kubernetes/kubecove.yaml) runs the same server binary; there is no separate Kubernetes backend. It mounts a kubeconfig whose `in-cluster` context uses a projected, rotating ServiceAccount token and the cluster CA. It runs in token mode and is reached with `kubectl port-forward`.

1. Push the image to a registry your k0s workers can pull. Replace `kubecove-server:local` in the example with that image reference, preferably by digest. Add an image pull secret if the registry requires one.
2. Create namespace `kubecove` and a PersistentVolumeClaim named `kubecove-data` in it. Choose storage class and capacity for your cluster. The volume must support a writable filesystem and `fsGroup`. The server creates its own private child directory and does not accept a group-readable volume root as its credential directory. k0s storage depends on your CSI provisioner; see [k0s storage](https://docs.k0sproject.io/stable/storage/).
3. Review the RBAC binding, then apply the example:

   ```sh
   kubectl apply -f deploy/kubernetes/kubecove.yaml
   kubectl -n kubecove rollout status deployment/kubecove
   ```

4. Forward the pod's loopback listener to your laptop:

   ```sh
   kubectl -n kubecove port-forward deployment/kubecove 8080:8080
   ```

5. Read the current server token privately:

   ```sh
   kubectl -n kubecove exec deployment/kubecove -c kubecove -- cat /data/kubecove/server-token
   ```

   Open `http://localhost:8080` and paste the token while the forward runs. Treat it as a password and keep it out of chat, shell history and diagnostic output. The token file uses mode `0600` and changes on every server restart. The deployment discards stdout because the startup line contains the token; stderr still shows errors.

6. For the desktop app, open Settings → Remote server and use `http://localhost:8080` with the same token. Desktop port forwards listen on the laptop.

The example runs one replica with the `Recreate` strategy, because the server has one active owner and one writer for its data directory. `fsGroupChangePolicy: OnRootMismatch` avoids recursive ownership changes when the volume root already matches. Check this with your storage driver; drivers that manage mount groups handle permissions themselves. See [Kubernetes volume permissions](https://kubernetes.io/docs/tasks/configure-pod-container/security-context/#configure-volume-permission-and-ownership-change-policy-for-pods). The example creates no Service, Ingress, NodePort or LoadBalancer. KubeCove itself deploys nothing; the operator applies these manifests.

## Cloudflare Tunnel and Access

Cloudflare Tunnel gives browsers an HTTPS route without a publicly reachable origin. A `cloudflared` container in the same pod reaches KubeCove's loopback listener. See [Cloudflare's Kubernetes guide](https://developers.cloudflare.com/tunnel/guides/kubernetes/) for the tunnel and its [Access setup](https://developers.cloudflare.com/cloudflare-one/setup/secure-private-apps/) for identity policies.

An HTTPS `--public-origin` requires native TLS on the server, including on pod loopback:

- Configure the server's certificate, key and exact public hostname.
- Point the tunnel at the HTTPS loopback origin with certificate verification enabled. Match the origin server name and trust chain to the certificate.
- Preserve the public Host header and allow WebSockets.
- Do not use `noTLSVerify` or weaken Host and Origin checks.
- Store tunnel credentials and TLS keys in Kubernetes Secrets, never in ConfigMaps or image layers.

To make Access the KubeCove sign-in, add both options to the server's TLS arguments:

```text
--cloudflare-access-team YOUR_TEAM_NAME
--cloudflare-access-aud YOUR_APPLICATION_AUD
```

Use the team name from `https://YOUR_TEAM_NAME.cloudflareaccess.com` and the application's AUD tag, not its UUID. The server verifies the Access JWT on each request against that team's signing keys, issuer and audience. Keep cloudflared's own Access validation enabled. The pod needs outbound HTTPS to the team's `/cdn-cgi/access/certs` endpoint to refresh signing keys. If refresh fails, the server denies access rather than using stale keys. See [Cloudflare JWT validation](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/).

After Access sign-in, KubeCove opens without a token form. The server creates no startup token or KubeCove authentication cookie in this mode, and an old token file does not authenticate. Cloudflare's policy decides who can sign in. Omit both options to keep token mode.

## OpenID Connect with Authentik

OIDC replaces token or Access sign-in with your identity provider. This example uses [Authentik](https://docs.goauthentik.io/add-secure-apps/providers/oauth2/) and keeps Cloudflare Tunnel as the HTTPS transport. Any provider that meets [ADR 0022](../docs/decisions/0022-oidc-server-sign-in.md) works the same way.

1. In Authentik, create an OAuth2/OpenID provider and application for KubeCove:
   - Client type: confidential.
   - Redirect URI: strict, `https://<kubecove-host>/api/v1/oidc/callback`.
   - Signing key: an RSA key, so ID tokens use RS256.
   - Bind the application to the users or group who should share this server's Kubernetes access. Authentik [passes targets with no applicable bindings](https://docs.goauthentik.io/customize/policies/bindings/#no-bindings-means-pass), so add at least one.
2. Store the client secret in a Kubernetes Secret and mount it as a read-only file in the server container.
3. Replace any Access options on the server with:

   ```text
   --oidc-issuer https://<authentik-host>/application/o/<application-slug>/
   --oidc-client-id <client-id>
   --oidc-client-secret-file /path/to/mounted/client-secret
   --oidc-login-timeout <duration>
   ```

   The issuer must exactly match the `issuer` in Authentik's discovery document, including the trailing slash. Choose the login deadline yourself; there is no default. The pod needs outbound HTTPS to the Authentik host.
4. Keep the tunnel's verified TLS, exact hostname and WebSocket support unchanged.
5. Verify OIDC before changing the Cloudflare Access application:
   - An unauthenticated session request returns 401.
   - A login request from another Origin returns 403.
   - An assigned user completes Authentik sign-in and reaches the KubeCove workspace launcher with contexts and namespaces.
   - An unassigned user is refused.
6. Only after these checks pass, remove the separate Cloudflare Access gate for this hostname, or switch its policy to bypass. Until then, keep it as the outer sign-in.

The tunnel's Access gate and KubeCove's sign-in mode are separate. With OIDC, KubeCove itself requires sign-in, and the server must not also be started with Access options. Keeping the outer gate until the checks pass avoids a period where an unverified Authentik assignment is the only barrier.

## Desktop clients

The desktop app authenticates only to token-mode servers. It does not perform Cloudflare Access or OIDC sign-in, so use a browser with those deployments. For desktop access to a cluster deployment, run a token-mode server and reach it through `kubectl port-forward` as in the k0s example. A token-mode server with TLS must present a certificate the laptop trusts for the hostname it connects to. Do not disable certificate verification.

## Recorded deployment evidence

Operator deployments of unpublished images ran in k0s between 2026-10-10 and 2026-10-11. They used native loopback TLS behind a Cloudflare Tunnel sidecar, first with Cloudflare Access sign-in and then with Authentik OIDC. Live Authentik sign-in reached the workspace launcher and loaded cluster contexts and namespaces. Unauthenticated session creation returned 401 and cross-origin login returned 403. See [server verification](../src-tauri/server/verification.md) for image digests, earlier read-only RBAC checks and remaining gaps.

The full isolated Kubernetes E2E suite and hosted cross-platform release builds have not been verified for these images. No registry image or product release has been published.
