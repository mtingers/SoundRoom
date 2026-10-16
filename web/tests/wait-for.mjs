/** Wait for observed async protocol progress, bounded under concurrent compiler load. */
export async function waitFor(predicate, description = 'condition', timeout = 5000) {
  const deadline = performance.now() + timeout;
  while (!predicate()) {
    if (performance.now() >= deadline) throw new Error(`Timed out waiting for ${description}.`);
    await new Promise(resolve => setTimeout(resolve, 1));
  }
}
