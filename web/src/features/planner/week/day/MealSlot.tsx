export function MealSlot({ meal, recipe }: { meal: string; recipe?: string }) {
  return (
    <div className={recipe ? "slot" : "slot slot--empty"} aria-label={meal}>
      {recipe ?? "Drop a recipe here"}
    </div>
  );
}
