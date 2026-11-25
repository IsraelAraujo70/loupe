import { MealSlots } from "./day/MealSlots";

const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

export function WeekView() {
  return (
    <section className="week">
      {DAYS.map((day) => (
        <MealSlots key={day} day={day} />
      ))}
    </section>
  );
}
