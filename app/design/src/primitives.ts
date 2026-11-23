/**
 * The shared controls of ui.css as components. In the app they are plain classes on plain
 * elements (`<button class="btn accent">`), so these shells only put the classes on; markup
 * with the same classes looks and behaves the same.
 */
import { R } from './wrap';

/* eslint-disable @typescript-eslint/no-explicit-any */
type Props = Record<string, any>;

export function makePrimitives(Icon: (p: Props) => unknown) {
  const icon = (name: unknown, size = 16) => (name ? R().createElement(Icon, { name, size }) : null);

  function Button({ variant = 'standard', icon: ic, disabled, children, onClick, style, title }: Props) {
    const cls = 'btn' + (variant === 'accent' ? ' accent' : variant === 'danger' ? ' danger' : '');
    return R().createElement('button', { className: cls, disabled: !!disabled, onClick, style, title, type: 'button' }, icon(ic), children != null ? R().createElement('span', { className: 'lbl' }, children) : null);
  }

  function SubtleButton({ icon: ic, label, children, disabled, pressed, onClick, style }: Props) {
    return R().createElement(
      'button',
      { className: 'sbtn', disabled: !!disabled, onClick, style, type: 'button', title: label, 'aria-label': children == null ? label : undefined, 'aria-pressed': pressed == null ? undefined : !!pressed },
      icon(ic),
      children != null ? R().createElement('span', null, children) : null,
    );
  }

  function Card({ children, padding = 16, style }: Props) {
    return R().createElement('div', { className: 'card', style: { padding, ...(style ?? {}) } }, children);
  }

  function Acrylic({ children, padding = 4, radius = 8, style }: Props) {
    return R().createElement('div', { className: 'acrylic', style: { padding, borderRadius: radius, ...(style ?? {}) } }, children);
  }

  for (const [n, c] of Object.entries({ Button, SubtleButton, Card, Acrylic })) (c as { displayName?: string }).displayName = n;
  return { Button, SubtleButton, Card, Acrylic };
}
