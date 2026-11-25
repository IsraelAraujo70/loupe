import type { RecipeSummary } from "../api/client";
import "./RecipeCard.css";

export function RecipeCard({ recipe }: { recipe: RecipeSummary }) {
  return (
    <article className="recipe-card">
      <h2 className="recipe-card__title">{recipe.name}</h2>
      <p className="recipe-card__servings">Serves {recipe.servings}</p>
    </article>
  );
}
