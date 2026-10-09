/** Keyboard activation for click targets promoted to `role="button"`. */

import type { KeyboardEvent } from "react";

/**
 * Keydown handler for click targets promoted to `role="button"`:
 * activates on Enter/Space like a native button. Ignores keys bubbling
 * up from nested interactive children so their own activation does not
 * double-fire the container's action.
 */
export function activationKeyHandler(action: () => void) {
  return (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    if (event.target !== event.currentTarget) return;
    event.preventDefault();
    action();
  };
}
