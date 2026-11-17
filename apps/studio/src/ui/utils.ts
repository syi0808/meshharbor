/** Join the small, source-owned class variants used by the Studio UI kit. */
export function cn(...classes: Array<string | false | null | undefined>): string {
  return classes.filter(Boolean).join(' ');
}
