import '@testing-library/jest-dom/vitest';

/*
 * jsdom ships no `matchMedia`, and the shell picks its layout from one. Tests
 * get a stub that reports the phone layout and never announces a change, so a
 * component under test renders the same way on every run.
 */
if (typeof window !== 'undefined' && typeof window.matchMedia !== 'function') {
    window.matchMedia = (query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
        addListener: () => undefined,
        removeListener: () => undefined,
        dispatchEvent: () => false,
    });
}
