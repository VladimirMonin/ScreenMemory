# Private LM Studio runtime notices

The original published @lmstudio/sdk 2.0.0 archive is preserved at
`vendor/sdk-2.0.0.tgz`; provenance and exact transitive sources/integrities are in
`DEPENDENCIES.json` and `pnpm-lock.yaml`. Upstream gitHead:
`f21fff9f316b39436860129bc3eef887c31c3cb7`.

**License discrepancy:** npm/package metadata declares MIT, but the actual SDK
archive LICENSE is Apache License 2.0. `vendor/SDK-LICENSE` preserves its full text
with only CRLF normalized to LF for checkout formatting. The original archive
LICENSE bytes remain unchanged in the checked tgz. Staging preserves upstream
package LICENSE/README/source and this notice. This is not an MIT-only claim or a legal/public-redistribution clearance.
No upstream NOTICE file was present in the inspected SDK archive.

Runtime dependency LICENSE files remain beside their package sources in shipped
`node_modules`; zod-to-json-schema uses ISC, other inspected transitives use MIT.
Node 24.13.0 is copied from the already pinned private runtime, together with its
full `NODE-LICENSE` (Node and bundled components). Neither LM Studio nor model
weights are redistributed or downloaded by this package.

The bridge adapter is ScreenMemory code, not a modification of SDK source.
All installation uses the frozen lock and `--ignore-scripts`. Dependency source
preflight found no install/preinstall/postinstall hooks or native executables;
there are no bufferutil/utf-8-validate optional native packages in the closure.
