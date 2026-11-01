# Definition of done

A change is done when all of these hold. The pull request says which checks ran.

1. **It does what was asked.** The acceptance criteria are met, or the PR says what is left.
2. **It fits the architecture.** Rules live in the domain crates, commands in the engine, screens
   in `ui-egui` or the web app; `cargo xtask layers` passes. No unrelated changes.
3. **It is tested.**
   - Changed logic has unit tests; spatial results use known coordinates and a tolerance.
   - Changed boundaries (files, server routes, PostgreSQL, the web API) have integration tests.
   - A fixed bug has a test that fails without the fix.
4. **The checks pass, and the PR lists them.**
   - `cargo xtask ci` (format, clippy, tests, layers).
   - `npm run check` in `apps/geoflux-web` when the web app changed.
   - A `ui_shot` screenshot or a browser screenshot when a screen changed.
   - Only checks that actually ran are listed.
5. **Data is safe.**
   - Source files are untouched.
   - Writes are atomic.
   - Every edit undoes in one step.
   - Destructive actions ask first.
   - Failures leave nothing half-done.
6. **Resources are released.**
   - Long work runs off the UI thread, with progress and Cancel.
   - Files, threads, temporary files and GPU objects are cleaned up on success, failure and cancel.
7. **The server is safe.**
   - Every route checks permission and has a body limit.
   - No secrets, paths or internal errors reach the client.
8. **The text is ready.**
   - Interface text is translatable (`tl!`).
   - New commands are listed in `docs/geoflux.md`.
   - A significant decision is added to `docs/decisions.md`.
9. **Limits are disclosed.** Anything known not to work yet is written in the PR.
