export interface RecipeSummary {
  id: string;
  name: string;
  servings: number;
}

const BASE_URL = "/api";

export async function listRecipes(): Promise<RecipeSummary[]> {
  const response = await fetch(`${BASE_URL}/recipes`);
  if (!response.ok) {
    throw new Error(`recipes failed: ${response.status}`);
  }
  return response.json();
}

export async function getRecipe(id: string): Promise<RecipeSummary> {
  const response = await fetch(`${BASE_URL}/recipes/${id}`);
  if (!response.ok) {
    throw new Error(`recipe ${id} failed: ${response.status}`);
  }
  return response.json();
}
