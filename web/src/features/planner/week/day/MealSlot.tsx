import { useState } from "react";

export function MealSlot({ meal, recipe }: { meal: string; recipe?: string }) {
  const [metric, setMetric] = useState(true);
  return (
    <div className={recipe ? "slot" : "slot slot--empty"} aria-label={meal}>
      {recipe ?? "Drop a recipe here"}
      {recipe && (
        <button type="button" onClick={() => setMetric(!metric)}>
          {metric ? "Show cups" : "Show grams"}
        </button>
      )}
    </div>
  );
}
