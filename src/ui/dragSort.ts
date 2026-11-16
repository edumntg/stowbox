/** Drag-to-reorder for a row of tabs. Each sortable child carries
 * `data-sort="<key>"`; dragging one past the middle of a neighbour calls
 * `onMove(dragged, neighbour)`, and the store reorders the row. Pointer
 * events rather than HTML drag-and-drop: on Windows, Tauri takes the
 * webview's drop events for dropped files. Returns the cleanup. */
export function dragSort(row: HTMLElement, onMove: (from: string, to: string) => void): () => void {
  let key: string | null = null;
  let el: HTMLElement | null = null;
  let startX = 0;
  let pointer = 0;
  let dragging = false;
  /** The click that ends a drag must not also select the tab. */
  let swallowClickUntil = 0;

  const down = (e: PointerEvent) => {
    if (e.button !== 0) return;
    const target = e.target as Element;
    const item = target.closest<HTMLElement>("[data-sort]");
    if (!item || item.parentElement !== row || target.closest(".tab-close")) return;
    key = item.dataset.sort ?? null;
    el = item;
    startX = e.clientX;
    pointer = e.pointerId;
    dragging = false;
  };
  const move = (e: PointerEvent) => {
    if (!key || !el || e.pointerId !== pointer) return;
    if (!dragging) {
      if (Math.abs(e.clientX - startX) < 5) return;
      dragging = true;
      row.setPointerCapture(pointer);
      el.classList.add("dragging");
    }
    const mine = el.getBoundingClientRect();
    for (const other of row.querySelectorAll<HTMLElement>(":scope > [data-sort]")) {
      if (other === el) continue;
      const r = other.getBoundingClientRect();
      if (e.clientX < r.left || e.clientX >= r.right) continue;
      // Past the neighbour's middle, so the two do not swap back and forth.
      const mid = r.left + r.width / 2;
      if (r.left > mine.left ? e.clientX > mid : e.clientX < mid) onMove(key, other.dataset.sort!);
      break;
    }
  };
  const up = (e: PointerEvent) => {
    if (e.pointerId !== pointer) return;
    if (dragging) {
      swallowClickUntil = performance.now() + 100;
      el?.classList.remove("dragging");
      if (row.hasPointerCapture(pointer)) row.releasePointerCapture(pointer);
    }
    key = null;
    el = null;
    dragging = false;
  };
  const click = (e: MouseEvent) => {
    if (performance.now() < swallowClickUntil) {
      e.stopPropagation();
      e.preventDefault();
    }
  };

  row.addEventListener("pointerdown", down);
  row.addEventListener("pointermove", move);
  row.addEventListener("pointerup", up);
  row.addEventListener("pointercancel", up);
  row.addEventListener("click", click, true);
  return () => {
    row.removeEventListener("pointerdown", down);
    row.removeEventListener("pointermove", move);
    row.removeEventListener("pointerup", up);
    row.removeEventListener("pointercancel", up);
    row.removeEventListener("click", click, true);
  };
}
