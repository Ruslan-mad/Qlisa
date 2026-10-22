/** Holds startup project-open drains until all backend event listeners exist. */
export function createProjectOpenStartupGate(drain: () => void) {
  let ready = false;
  let pending = false;
  let disposed = false;

  return {
    requestDrain() {
      if (disposed) return;
      if (!ready) {
        pending = true;
        return;
      }
      drain();
    },
    markListenersReady() {
      if (disposed || ready) return;
      ready = true;
      if (pending) {
        pending = false;
        drain();
      }
    },
    dispose() {
      disposed = true;
      pending = false;
    },
  };
}
