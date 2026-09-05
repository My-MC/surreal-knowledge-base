import { GlobalRegistrator } from "@happy-dom/global-registrator";

// Register window/document/... globals so Testing Library can render under
// `bun test` (bun provides no DOM by default — bun docs "test/dom").
GlobalRegistrator.register();

// sigma.js reads WebGL2RenderingContext/WebGLRenderingContext constants at
// module load; happy-dom does not define these globals. A Proxy stub keeps
// module-scope constant lookups working (values are irrelevant — the
// GraphView tests fake the GL context itself).
function stubWebGLGlobal(name: "WebGLRenderingContext" | "WebGL2RenderingContext"): void {
  if (typeof globalThis[name] !== "undefined") return;
  const stub = new Proxy(class {}, {
    get: (target, prop) => (prop in target ? Reflect.get(target, prop) : 1),
  });
  Object.defineProperty(globalThis, name, { value: stub, writable: true, configurable: true });
}

stubWebGLGlobal("WebGLRenderingContext");
stubWebGLGlobal("WebGL2RenderingContext");
