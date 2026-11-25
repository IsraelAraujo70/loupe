import { MealSlot } from "./MealSlot";

const MEALS = ["breakfast", "lunch", "dinner"] as const;

export function MealSlots({ day }: { day: string }) {
  return (
    <div className="day">
      <h3>{day}</h3>
      {MEALS.map((meal) => (
        <MealSlot key={meal} meal={meal} />
      ))}
    </div>
  );
}
