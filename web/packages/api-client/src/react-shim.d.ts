// Minimal React hook surface for typechecking useChatStream.ts without a react
// dependency: the consuming apps own the real react package. Scoped to this
// package via tsconfig paths — never resolved at runtime.
declare function useState<S>(
  initialState: S | (() => S),
): [S, (update: S | ((prev: S) => S)) => void];
declare function useRef<T>(initialValue: T): { current: T };
declare function useEffect(effect: () => (() => void) | undefined, deps: readonly unknown[]): void;
declare function useCallback<T extends (...args: never[]) => unknown>(
  callback: T,
  deps: readonly unknown[],
): T;

export { useCallback, useEffect, useRef, useState };
