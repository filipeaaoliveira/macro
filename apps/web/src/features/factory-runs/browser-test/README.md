# Runs browser regression

From `apps/web`, run:

```sh
bunx playwright test --config src/features/factory-runs/browser-test/playwright.config.ts
```

Set `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` to use an existing Chromium, and
`FACTORY_RUNS_SCREENSHOT_DIR` to save desktop and phone screenshots.
`FACTORY_RUNS_BROWSER_PORT` overrides port 3019; the harness owns its Vite
server.

The fixture mounts the real side panel, Runs section and Run dialog on an
in-memory factory (`main.tsx`): one Run waits at a Gate, one runs, two have
ended. The test answers the Gate with a note, cancels the Run after its inline
confirmation, and starts a new Run from a chosen Flow.
