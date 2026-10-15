/** Marker anchors and viewBox of the G502 X top view (assets/design/mouse-anchors.json). */

export interface Point {
  x: number;
  y: number;
}

export const G502X_VIEWBOX = { width: 200, height: 320 } as const;

/** Marker positions per slot index 0..10 (G1..G11), in viewBox units. */
export const G502X_SLOTS: Record<number, Point> = {
  0: { x: 72, y: 70 }, // G1 left primary
  1: { x: 134, y: 70 }, // G2 right primary
  2: { x: 100, y: 74 }, // G3 wheel
  3: { x: 41, y: 214 }, // G4 rear side button
  4: { x: 44, y: 145 }, // G5 front side button (sniper)
  5: { x: 40, y: 179 }, // G6 middle side button
  6: { x: 90.5, y: 62 }, // G7 wheel tilt left
  7: { x: 109.5, y: 86 }, // G8 wheel tilt right
  8: { x: 100, y: 113 }, // G9 button behind the wheel
  9: { x: 47, y: 68 }, // G10 small button, front
  10: { x: 47, y: 100 }, // G11 small button, rear
};
