/** Class-name helper shared by the UI components. */

import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/** Joins class names and resolves conflicting Tailwind utilities, last one winning. */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
