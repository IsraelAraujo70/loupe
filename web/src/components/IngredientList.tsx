export interface IngredientLine {
  ingredient: string;
  value: number;
  unit: string;
}

export function IngredientList({ lines }: { lines: IngredientLine[] }) {
  if (lines.length === 0) {
    return <p className="ingredient-list__empty">No ingredients yet.</p>;
  }
  return (
    <ul className="ingredient-list">
      {lines.map((line) => (
        <li key={line.ingredient}>
          {line.value} {line.unit} {line.ingredient}
        </li>
      ))}
    </ul>
  );
}
