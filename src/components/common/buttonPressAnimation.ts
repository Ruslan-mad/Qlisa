const BUTTON_PRESS_KEYFRAMES: Keyframe[] = [
  { transform: "scale(1)", filter: "brightness(1)" },
  { transform: "scale(0.88)", filter: "brightness(1.7)", offset: 0.35 },
  { transform: "scale(1)", filter: "brightness(1)" },
];

const BUTTON_PRESS_OPTIONS: KeyframeAnimationOptions = {
  duration: 260,
  easing: "cubic-bezier(.2,.7,.3,1)",
};

export function animateButtonPress(button: HTMLButtonElement | null): void {
  if (!button || button.disabled) return;
  button.animate(BUTTON_PRESS_KEYFRAMES, BUTTON_PRESS_OPTIONS);
}
