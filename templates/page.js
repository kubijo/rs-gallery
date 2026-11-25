// @ts-check
// The page reads without any of this: it opens what a link points at,
// seeks a test's video to a line of its log, and looks closely at images.

/**
 * How the image in the lightbox is placed: its corner on the stage, in pixels, and its scale.
 *
 * @typedef {{ x: number, y: number, zoom: number }} View
 */

/**
 * The element `selector` finds in `root`, which the page always has.
 *
 * @template {Element} T
 * @param {ParentNode} root
 * @param {string} selector
 * @param {new () => T} kind
 * @returns {T}
 */
function findIn(root, selector, kind) {
    const found = root.querySelector(selector);
    if (!(found instanceof kind)) {
        throw new Error(`BUG: no ${kind.name} at ${selector}`);
    }
    return found;
}

for (const time of document.querySelectorAll('time')) {
    time.title = time.textContent;
    time.textContent = new Date(time.dateTime).toLocaleString(undefined, {
        dateStyle: 'medium',
        timeStyle: 'short',
    });
}

const all = findIn(document, '#show-all', HTMLInputElement);

/**
 * Open what a link points at, and show it if the filter does not.
 *
 * @returns {void}
 */
function openTarget() {
    const target = document.querySelector(':target');
    if (!target) {
        return;
    }
    if (target instanceof HTMLDetailsElement) {
        target.open = true;
    }
    if (!target.checkVisibility()) {
        all.checked = true;
        target.scrollIntoView();
    }
}
addEventListener('hashchange', openTarget);
// A link to what is the target already changes no hash.
addEventListener('click', event => {
    if (event.target instanceof Element && event.target.closest('a[href^="#"]')) {
        setTimeout(openTarget);
    }
});
openTarget();

/**
 * It asks for a tab or a window of the link's own, or to download it.
 *
 * @param {MouseEvent} event
 * @returns {boolean}
 */
function opensElsewhere(event) {
    return event.ctrlKey || event.metaKey || event.shiftKey || event.altKey;
}

// A video loads no more than is looked at: a page of very many could not play them all.
const seen = new IntersectionObserver(entries => {
    for (const { isIntersecting, target } of entries) {
        if (isIntersecting && target instanceof HTMLVideoElement) {
            target.preload = 'metadata';
            seen.unobserve(target);
        }
    }
});
for (const video of document.querySelectorAll('video')) {
    seen.observe(video);
}

for (const body of document.querySelectorAll('.body:has(a.seek)')) {
    const video = findIn(body, 'video', HTMLVideoElement);
    /** @type {NodeListOf<HTMLElement>} */
    const lines = body.querySelectorAll('.line');
    for (const line of lines) {
        const link = line.querySelector('a.seek');
        if (!(link instanceof HTMLAnchorElement)) {
            continue;
        }
        link.addEventListener('click', event => {
            if (opensElsewhere(event)) {
                return;
            }
            event.preventDefault();
            for (const other of lines) {
                other.classList.toggle('now', other === line);
                other.querySelector('a.seek')?.removeAttribute('aria-current');
            }
            link.setAttribute('aria-current', 'true');
            video.pause();
            video.currentTime = Number(link.dataset.at);
            video.scrollIntoView({ block: 'nearest' });
        });
    }
}

const lightbox = findIn(document, '#lightbox', HTMLDialogElement);
const stage = findIn(lightbox, '.stage', HTMLDivElement);
const shown = findIn(stage, 'img', HTMLImageElement);
const names = findIn(lightbox, '.names', HTMLFieldSetElement);
const which = findIn(lightbox, '.which', HTMLSpanElement);
const zoomed = findIn(lightbox, '.zoom', HTMLSpanElement);
const flicker = findIn(lightbox, '.flicker', HTMLButtonElement);

/**
 * Links to the images of the check looked at, what was expected of it first.
 *
 * @type {HTMLAnchorElement[]}
 */
let images = [];
/** Which of them is shown. */
let at = 0;
/** @type {View} */
let view = { x: 0, y: 0, zoom: 1 };
/** The view is of an image, and not of one that is still to load. */
let fitted = false;
/**
 * What alternates two of them, if anything does.
 *
 * @type {ReturnType<typeof setInterval> | undefined}
 */
let flickering;

/**
 * Where each pointer that holds the image is, in the window.
 *
 * @type {Map<number, { x: number, y: number }>}
 */
const held = new Map();

/** @returns {void} */
function place() {
    shown.style.transform = `translate(${view.x}px, ${view.y}px) scale(${view.zoom})`;
    shown.classList.toggle('close-up', view.zoom >= 2);
    zoomed.textContent = `${Math.round(view.zoom * 100)} %`;
}

/**
 * Show all of the image, no larger than it is.
 *
 * @returns {void}
 */
function fit() {
    const { clientWidth: width, clientHeight: height } = stage;
    fitted = lightbox.open && shown.naturalWidth > 0;
    if (!fitted) {
        view = { x: 0, y: 0, zoom: 1 };
        place();
        return;
    }
    const zoom = Math.min(width / shown.naturalWidth, height / shown.naturalHeight, 1);
    view = {
        x: (width - shown.naturalWidth * zoom) / 2,
        y: (height - shown.naturalHeight * zoom) / 2,
        zoom,
    };
    place();
}

/**
 * Show another image, as closely and at the same place as the one before,
 * or all of the first that loads.
 *
 * @param {number} next which, going round at either end
 * @returns {Promise<void>}
 */
async function show(next) {
    at = (next + images.length) % images.length;
    const image = images[at];
    if (!image) {
        return;
    }
    shown.src = image.href;
    shown.alt = image.ariaLabel ?? '';
    for (const [index, name] of [...names.children].entries()) {
        name.setAttribute('aria-pressed', String(index === at));
    }
    await shown.decode().catch(() => {});
    if (!fitted && image === images[at]) {
        fit();
    }
}

/** @returns {void} */
function stopFlicker() {
    clearInterval(flickering);
    flickering = undefined;
    flicker.setAttribute('aria-pressed', 'false');
}

/**
 * Alternate the first two images, what was expected and what was seen, or stop.
 *
 * @returns {void}
 */
function toggleFlicker() {
    if (flickering !== undefined) {
        stopFlicker();
        sayWhich();
        return;
    }
    flicker.setAttribute('aria-pressed', 'true');
    show(0);
    flickering = setInterval(() => show(at === 0 ? 1 : 0), 400);
    const [first, second] = images.map(image => image.ariaLabel);
    which.textContent = `${first} and ${second}, alternating`;
}

/**
 * Zoom, what is at a point of the stage staying there.
 *
 * @param {number} x
 * @param {number} y
 * @param {number} factor
 * @returns {void}
 */
function zoomAround(x, y, factor) {
    const zoom = Math.min(Math.max(view.zoom * factor, 0.05), 40);
    const scale = zoom / view.zoom;
    view = { x: x - (x - view.x) * scale, y: y - (y - view.y) * scale, zoom };
    place();
}

/**
 * Zoom, what is in the middle of the stage staying there.
 *
 * @param {number} factor
 * @returns {void}
 */
function zoomBy(factor) {
    zoomAround(stage.clientWidth / 2, stage.clientHeight / 2, factor);
}

/**
 * @param {number} x
 * @param {number} y
 * @returns {void}
 */
function panBy(x, y) {
    view = { ...view, x: view.x + x, y: view.y + y };
    place();
}

/**
 * @param {number} next
 * @returns {void}
 */
function step(next) {
    stopFlicker();
    show(next);
    sayWhich();
}

/**
 * Tell a screen reader which image is shown.
 *
 * @returns {void}
 */
function sayWhich() {
    which.textContent = `${shown.alt}, ${at + 1} of ${images.length}`;
}

/**
 * Look at the images of a check.
 *
 * @param {HTMLAnchorElement[]} set
 * @param {number} first
 * @returns {Promise<void>}
 */
async function look(set, first) {
    images = set;
    const buttons = images.map((image, index) => {
        const button = document.createElement('button');
        button.type = 'button';
        button.textContent = image.ariaLabel;
        button.addEventListener('click', () => step(index));
        return button;
    });
    names.replaceChildren(...buttons);
    flicker.hidden = images.length < 2;
    fitted = false;
    lightbox.showModal();
    await show(first);
    if (lightbox.open) {
        sayWhich();
    }
}

for (const check of document.querySelectorAll('.images')) {
    const links = [...check.querySelectorAll('a')];
    for (const [index, link] of links.entries()) {
        link.addEventListener('click', event => {
            if (opensElsewhere(event)) {
                return;
            }
            event.preventDefault();
            look(links, index);
        });
    }
}

flicker.addEventListener('click', toggleFlicker);
findIn(lightbox, '.fit', HTMLButtonElement).addEventListener('click', fit);
findIn(lightbox, '.close', HTMLButtonElement).addEventListener('click', () => lightbox.close());
lightbox.addEventListener('close', () => {
    stopFlicker();
    held.clear();
});
addEventListener('resize', () => {
    if (lightbox.open) {
        fit();
    }
});

lightbox.addEventListener('keydown', event => {
    // The browser's own shortcuts.
    if (event.ctrlKey || event.metaKey || event.altKey) {
        return;
    }
    const far = 40;
    const index = Number(event.key) - 1;
    if (event.shiftKey && event.key === 'ArrowRight') {
        panBy(-far, 0);
    } else if (event.shiftKey && event.key === 'ArrowLeft') {
        panBy(far, 0);
    } else if (event.shiftKey && event.key === 'ArrowDown') {
        panBy(0, -far);
    } else if (event.shiftKey && event.key === 'ArrowUp') {
        panBy(0, far);
    } else if (event.key === 'ArrowRight') {
        step(at + 1);
    } else if (event.key === 'ArrowLeft') {
        step(at - 1);
    } else if (event.key === '+' || event.key === '=') {
        zoomBy(1.25);
    } else if (event.key === '-') {
        zoomBy(1 / 1.25);
    } else if (event.key === 'f' && images.length > 1) {
        toggleFlicker();
    } else if (event.key === '0') {
        fit();
    } else if (index >= 0 && index < images.length) {
        step(index);
    } else {
        return;
    }
    event.preventDefault();
});

stage.addEventListener(
    'wheel',
    event => {
        event.preventDefault();
        const { left, top } = stage.getBoundingClientRect();
        zoomAround(event.clientX - left, event.clientY - top, 1.0015 ** -event.deltaY);
    },
    { passive: false },
);

stage.addEventListener('pointerdown', event => {
    if (event.button !== 0) {
        return;
    }
    event.preventDefault();
    stage.setPointerCapture(event.pointerId);
    held.set(event.pointerId, { x: event.clientX, y: event.clientY });
});

stage.addEventListener('pointermove', event => {
    const was = held.get(event.pointerId);
    if (!was) {
        return;
    }
    const now = { x: event.clientX, y: event.clientY };
    held.set(event.pointerId, now);
    const [other] = [...held].flatMap(([id, point]) => (id === event.pointerId ? [] : [point]));
    if (!other) {
        panBy(now.x - was.x, now.y - was.y);
        return;
    }
    // Two fingers pinch: what is under the still one stays there.
    const apart = Math.hypot(was.x - other.x, was.y - other.y);
    const { left, top } = stage.getBoundingClientRect();
    const factor = Math.hypot(now.x - other.x, now.y - other.y) / Math.max(apart, 1);
    zoomAround(other.x - left, other.y - top, factor);
});

stage.addEventListener('lostpointercapture', event => held.delete(event.pointerId));
