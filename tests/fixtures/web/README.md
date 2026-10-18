# Native web acceptance fixture

Serve or package `local-form.html` with its adjacent `mark.svg` as a browser-service-owned local resource. It has no external dependencies. This fixture is a test input, not evidence that a browser is integrated.

Native QEMU acceptance:

1. Open the page in a Ferrofluid-owned surface; confirm styled text and the blue image appear.
2. Confirm `document.documentElement.dataset.scriptReady` is `true` through test-only browser instrumentation.
3. Focus the name field using guest touch input, enter `Alloy` with the alloyOS soft keyboard, and press Greet. Confirm the visible result is `Hello, Alloy!`.
4. Scroll until the acceptance target appears, then return to the form. Confirm touch coordinates and focus remain correct at the configured display scale.
5. Terminate or hang the browser service. Verify system controls and another app remain responsive, then reopen the fixture in a recovered service.

A host browser rendering this fixture does not satisfy native acceptance. General HTTPS, origins, profiles and permissions need separate acceptance after the local page works.

Before browser integration, run `engine-smoke.js` with the native JavaScriptCore shell in QEMU. Require a successful exit and `ALLOYOS_JSC_SMOKE_PASS` after the promise microtask runs. A host JSC run only checks the fixture; it does not satisfy the guest engine gate.
