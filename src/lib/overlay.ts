/** Une boîte modale ou un menu est ouvert : la frappe lui revient. */
export function overlayOpen() {
  return document.querySelector("[aria-modal='true'], [role='menu']") !== null;
}
