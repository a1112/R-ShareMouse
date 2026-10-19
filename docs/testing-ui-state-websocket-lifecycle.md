# UI state WebSocket ownership regression

The browser UI state connector owns its socket event listeners and consumer callbacks until close, abort, remote close, protocol failure, or connection setup failure. Teardown removes all four listeners and the abort subscription, clears consumer references, and fences callbacks already queued by the platform. A failed initial subscribe rejects the connection attempt. Normal subscribe, envelope delivery, and full resync are preserved.

Run the focused synthetic checks from `apps/rshare-desktop-frontend`:

```sh
node --test --test-concurrency=1 src/app/ui-state-client.test.mjs src/app/ui-store.test.mjs src/app/ipc-frame.test.mjs src/app/ui-state-websocket-lifecycle.test.mjs
npm run build
```

The lifecycle test launches one small Node child process with `--expose-gc` to check consumer reachability. Its fake transport deliberately keeps 41 socket objects and 40 already queued message callbacks alive: 40 retired consumers must be collectible, while the live control remains retained. The `.node.mjs` fixture is invoked explicitly and is outside the default `*.test.mjs` test glob.

The original adapter retained 160 registered listeners and 40 retired consumers in that fixture; after teardown the expected counts are zero, with no late delivery. These are synthetic JS registration and reachability checks, not process RSS, whole-application RAM, real WebSocket/Tauri, native interaction, or dual-machine acceptance. Physical socket close remains best effort. The Tauri adapter, native input handling, daemon, reconnect timing, fallback polling, and stream protocol are unchanged.

This frontend has no configured TypeScript gate or `tsconfig.json`. The modified production `.mjs` module can be checked with an available TypeScript compiler using `--allowJs --checkJs --noEmit --target es2022 --module nodenext --lib es2022,dom --skipLibCheck`; that is a module-level JavaScript check. A static Vite build does not start the dev bridge or daemon.
