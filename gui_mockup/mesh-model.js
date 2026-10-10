/* Pure routing model for the design reference. No networking/cryptography.
 * Injected `authenticated` observations simulate successful native probes.
 * Never treat this model as an implementation of native relay transport.
 */
(function(root, factory) {
  const api = factory();
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.MeshModel = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, function() {
  const key = path => path.join('>');
  class Routes {
    constructor(nodes, source = 'you') {
      this.nodes = new Map(nodes.map(n => [n.id, n]));
      this.source = source;
      this.probes = new Map();
      this.active = new Map();
      this.maxAge = 10000;
      this.cooldown = 10000;
    }
    allowed(path) {
      return (path.length === 2 || path.length === 3) && path[0] === this.source
        && new Set(path).size === path.length
        && path.every(id => this.nodes.has(id) && this.nodes.get(id).online)
        && (path.length === 2 || this.nodes.get(path[1]).relay === true);
    }
    observe(path, rtt, now, authenticated = false) {
      if (!authenticated || !this.allowed(path) || !Number.isFinite(rtt) || rtt <= 0) return false;
      const k = key(path), prev = this.probes.get(k);
      // Duplicate/out-of-order observations must not count as extra validation.
      if (prev && now <= prev.at) return false;
      const fresh = prev && now - prev.at <= this.maxAge;
      this.probes.set(k, { path: [...path], rtt: fresh ? prev.rtt * .75 + rtt * .25 : rtt,
        samples: fresh ? prev.samples + 1 : 1, at: now });
      return true;
    }
    invalidate(id) {
      for (const [k, p] of this.probes) if (p.path.includes(id)) this.probes.delete(k);
    }
    choose(target, now) {
      const valid = p => p && this.allowed(p.path) && now - p.at >= 0
        && now - p.at <= this.maxAge && p.samples >= 3;
      const candidates = [...this.probes.values()].filter(p => p.path.at(-1) === target && valid(p))
        .sort((a,b) => a.rtt - b.rtt || a.path.length - b.path.length);
      const prior = this.active.get(target), current = prior && this.probes.get(prior.key);
      const best = candidates[0];
      if (!best) { this.active.delete(target); return null; }
      let chosen = valid(current) ? current : best;
      if (valid(current) && key(best.path) !== prior.key
        && now - prior.since >= this.cooldown
        && current.rtt - best.rtt >= Math.max(3, current.rtt * .2)) chosen = best;
      const changed = !prior || prior.key !== key(chosen.path);
      this.active.set(target, { key: key(chosen.path), since: changed ? now : prior.since });
      return { ...chosen, changed, via: chosen.path.length === 3 ? chosen.path[1] : null };
    }
  }
  return { Routes, key };
});
