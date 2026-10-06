import {useEffect, useState} from "react";

// A wheel delta is an event, not a held button. Only animate it briefly.
export function useInputPulse(sequence) {
  const [activeSequence, setActiveSequence] = useState(null);
  useEffect(() => {
    if (sequence == null) { setActiveSequence(null); return; }
    setActiveSequence(sequence);
    const timer = setTimeout(() => setActiveSequence(null), 180);
    return () => clearTimeout(timer);
  }, [sequence]);
  return sequence != null && activeSequence === sequence;
}
