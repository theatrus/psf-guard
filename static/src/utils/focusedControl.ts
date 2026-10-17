/** Whether the key lands on a control that Enter or Space presses, such as
 *  a button. A view's own Enter or Space shortcut leaves that press alone. */
export function pressesFocusedControl(event: KeyboardEvent): boolean {
  return event.target instanceof Element && event.target.closest(
    'button, a[href], summary, [role="button"], [role="link"], [role="tab"], [role="switch"], [role="checkbox"]',
  ) !== null;
}
