export interface RecipeSummary {
  id: string;
  name: string;
  servings: number;
}

const BASE_URL = "/api/v1";
const TIMEOUT_MS = 10_000;

export async function listRecipes(): Promise<RecipeSummary[]> {
  const response = await fetch(`${BASE_URL}/recipes`, {
    signal: AbortSignal.timeout(TIMEOUT_MS),
  });
  if (!response.ok) {
    throw new Error(`recipes failed: ${response.status}`);
  }
  return response.json();
}

export async function getRecipe(recipeId: string): Promise<RecipeSummary> {
  const response = await fetch(`${BASE_URL}/recipes/${recipeId}`);
  if (!response.ok) {
    throw new Error(`recipe ${recipeId} failed: ${response.status}`);
  }
  return response.json();
}
