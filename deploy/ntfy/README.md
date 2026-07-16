# ntfy — UnifiedPush broker for the shep companion (A3)

Self-hosted ntfy that pages the phone when an agent blocks. Personal, tailnet-only.

## Topology

```
shep server (mini) --[blocked transition]--> [notifications] exec
    = "SHEP_NTFY_PUBLISH_BASE=http://127.0.0.1:2587 shep bridge notify-push"
        -> reads <config>/push-endpoints.json (phone registered these over the bridge)
        -> POST JSON body to the loopback ntfy (co-located; avoids MagicDNS)
ntfy (Docker) --[UnifiedPush]--> ntfy Android app (distributor) --> Shep app
    -> Shep renders an actionable notification: Approve/Deny -> pane.send_keys y/n
```

- ntfy runs as a container (macOS ntfy bottle is client-only — no `serve`).
- A Tailscale sidecar publishes it at `https://ntfy.tail58187b.ts.net/` with a real
  cert. Backend binds `127.0.0.1:2587`; only the sidecar reaches it.
- shep publishes to the **loopback** ntfy (same box); the phone subscribes to the
  **tailnet HTTPS** URL. Same topics, different host — that's `SHEP_NTFY_PUBLISH_BASE`.

## Bring up / manage

```bash
docker compose -f deploy/ntfy/docker-compose.yml up -d      # up -d, not restart
docker compose -f deploy/ntfy/docker-compose.yml logs -f
curl -s --resolve ntfy.tail58187b.ts.net:443:$(tailscale ip -4 ntfy) \
     https://ntfy.tail58187b.ts.net/v1/health                # {"healthy":true}
```

Authkey lives at `~/.config/ntfy-tailscale/ts_authkey` (reusable, minted from the
tailscale OAuth client). Not a launchd service yet — same status as `shep bridge`.

## Phone setup (the A3 gate — Alex's manual step)

1. Install **ntfy** from F-Droid (the UnifiedPush distributor; no Google Play Services).
2. In ntfy → Settings → **Default server** = `https://ntfy.tail58187b.ts.net`. Ensure
   the phone is on the tailnet. (Optionally enable it as the UnifiedPush distributor;
   it registers automatically when the Shep app asks.)
3. Install the Shep APK (`app/build/outputs/apk/release/app-release.apk`) and pair.
4. Grant the notification permission when prompted. The Shep tab should flip from
   "no push distributor" to "push registered" with an endpoint.
5. Verify: block an agent on the mini (or run the manual publish test below). With the
   Shep app closed and the phone locked, a notification with **Approve/Deny** should
   arrive; Approve sends `y` to the pane.

## Manual publish test (no agent needed)

```bash
# after the phone has registered (endpoint in <config>/push-endpoints.json):
SHEP_NTFY_PUBLISH_BASE=http://127.0.0.1:2587 \
SHEP_NOTIFY_STATE=blocked SHEP_NOTIFY_AGENT=claude SHEP_NOTIFY_WORKSPACE=demo \
SHEP_NOTIFY_PANE_ID=1 SHEP_NOTIFY_MESSAGE="Allow tool: Bash(ls)?" \
shep bridge notify-push
```
