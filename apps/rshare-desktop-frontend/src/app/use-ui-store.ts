import { useCallback, useRef, useSyncExternalStore } from "react";

import {
  createUiStore,
  selectInputVisuals,
  selectTopologyProjection,
} from "./ui-store.mjs";

export const uiStateStore = createUiStore();

if (
  typeof window !== "undefined" &&
  (window as Window & { __rsharePerfEnableStoreAccess?: boolean })
    .__rsharePerfEnableStoreAccess
) {
  (
    window as Window & {
      __rsharePerfStoreAccess?: {
        store: typeof uiStateStore;
        selectInputVisuals: typeof selectInputVisuals;
        selectTopologyProjection: typeof selectTopologyProjection;
      };
    }
  ).__rsharePerfStoreAccess = {
    store: uiStateStore,
    selectInputVisuals,
    selectTopologyProjection,
  };
}

export function createUseUiStore(store: typeof uiStateStore) {
  return function useSelectedUiState<T>(
    selector: (state: ReturnType<typeof store.getState>) => T,
    equality: (left: T, right: T) => boolean = Object.is,
  ): T {
    const selectorRef = useRef(selector);
    const equalityRef = useRef(equality);
    const cacheRef = useRef<{ initialized: boolean; value: T | undefined }>({
      initialized: false,
      value: undefined,
    });
    selectorRef.current = selector;
    equalityRef.current = equality;

    const subscribe = useCallback(
      (notify: () => void) =>
        store.subscribe(
          (state) => selectorRef.current(state),
          (selected) => {
            cacheRef.current = { initialized: true, value: selected };
            notify();
          },
          (left, right) => equalityRef.current(left, right),
        ),
      [],
    );
    const getSnapshot = useCallback(() => {
      const selected = selectorRef.current(store.getState());
      const cache = cacheRef.current;
      if (
        cache.initialized &&
        equalityRef.current(cache.value as T, selected)
      ) {
        return cache.value as T;
      }
      cacheRef.current = { initialized: true, value: selected };
      return selected;
    }, []);

    return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  };
}

export const useUiStore = createUseUiStore(uiStateStore);
