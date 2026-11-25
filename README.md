# Pantry planner (Loupe test fixture)

> This branch is test data for [Loupe](https://github.com/IsraelAraujo70/loupe).
> Never merge it. See docs/TEST_FIXTURE.md on the main branch.

A small sample project: a recipe scaler, a pantry stock list, and a web
client that shows recipe cards. The code is generic example code.

## Layout

- `src/recipe`: recipes, parsing, and scaling to a number of servings.
- `src/pantry`: what is in stock and when it was bought.
- `src/catalog`: the ingredient catalog.
- `web`: the browser client.

## Running

Run `cargo test` for the library and `npm test` in `web` for the client.
