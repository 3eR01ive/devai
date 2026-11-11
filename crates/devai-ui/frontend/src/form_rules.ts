// The operator rules SHARED by the signature card and the QA gate (rework R3):
// exactly two rules that must not drift between surfaces.
//
//   * the non-empty rationale — the only condition the WINDOW adds to a human
//     act (empty or whitespace-only text never signs and never sends);
//   * the wording of an absent site — an unnamed site is said in words, never
//     silently dropped.
//
// No runtime imports here on purpose: the draft module is driven by
// `node --test`, and a leaf module keeps that harness free of Vue/Vite.

/** The non-empty rationale rule (REQ-SIGN-04 AC4, REQ-QG-02 AC2). */
export function rationaleValid(rationale: string): boolean {
  return rationale.trim().length > 0;
}

/** Site wording: an absent site is named in words, never silently dropped.
 * ONE wording for every surface — the first layer, the return draft and the
 * signature card all say «место не указано» (REQ-SAY-03 AC2). */
export function siteText(it: string | { site: string }): string {
  const site = typeof it === "string" ? it : it.site;
  return site.trim() ? site : "место не указано";
}
