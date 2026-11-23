const STYLE_ID = 'stremio-enhanced-spatial-nav-fix';
const PATCH_POLL_INTERVAL_MS = 100;
const PATCH_TIMEOUT_MS = 15000;

let initialized = false;
let patched = false;

function isCandidateVisibleForSpatialNav(el: unknown): boolean {
    if (!(el instanceof Element)) return false;
    if (el.getClientRects().length === 0) return false;

    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 && rect.height <= 0) return false;

    const style = window.getComputedStyle(el);
    if (
        style.visibility === 'hidden' ||
        style.visibility === 'collapse' ||
        style.opacity === '0'
    ) {
        return false;
    }

    return true;
}

function tryPatchSpatialNavigationSearch(): boolean {
    if (patched) return true;
    const proto = Element.prototype as any;
    const orig = proto.spatialNavigationSearch;
    if (typeof orig !== 'function') return false;
    if (orig.__isEnhancedPatched) {
        patched = true;
        return true;
    }

    const wrapped = function (this: Element, dir: any, opts?: any) {
        if (opts && Array.isArray(opts.candidates)) {
            const filtered = opts.candidates.filter(isCandidateVisibleForSpatialNav);
            opts = { ...opts, candidates: filtered };
        }
        return orig.call(this, dir, opts);
    };
    (wrapped as any).__isEnhancedPatched = true;
    proto.spatialNavigationSearch = wrapped;
    patched = true;
    return true;
}

function injectSpatialNavStyle(): void {
    if (document.getElementById(STYLE_ID)) return;
    const style = document.createElement('style');
    style.id = STYLE_ID;
    // --spatial-navigation-action is non-inherited in WICG spatial-navigation-polyfill
    style.textContent = `
*, *::before, *::after {
    --spatial-navigation-action: focus !important;
}
`;
    (document.head || document.documentElement).appendChild(style);
}

export function setupKeyboardNavigationFix(): void {
    if (initialized) return;
    initialized = true;

    injectSpatialNavStyle();

    if (tryPatchSpatialNavigationSearch()) return;

    const onReady = () => {
        injectSpatialNavStyle();
        tryPatchSpatialNavigationSearch();
    };
    window.addEventListener('DOMContentLoaded', onReady, { once: true });
    window.addEventListener('load', onReady, { once: true });

    const startedAt = Date.now();
    const timer = setInterval(() => {
        injectSpatialNavStyle();
        if (tryPatchSpatialNavigationSearch() || Date.now() - startedAt >= PATCH_TIMEOUT_MS) {
            clearInterval(timer);
        }
    }, PATCH_POLL_INTERVAL_MS);
}
