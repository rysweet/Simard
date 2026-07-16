# Gastronome — Recipe Design

Design a single, executable recipe as structured data the planner can cost,
scale, and schedule. A recipe is not prose — it is a portioned, priced,
timed object.

## Output Contract

Produce a JSON object matching `simard::gastronome::Recipe`:

```json
{
  "id": "chickpea-curry",
  "name": "Chickpea & Spinach Curry",
  "course": "main",
  "base_servings": 4,
  "dietary_tags": ["vegetarian", "vegan", "gluten-free", "dairy-free", "nut-free", "halal"],
  "ingredients": [
    {
      "ingredient": {
        "name": "chickpeas",
        "unit": "kg",
        "cost_per_unit": 2.5,
        "nutrition_per_unit": { "calories": 1640.0, "protein_g": 90.0, "carbs_g": 270.0, "fat_g": 26.0 }
      },
      "quantity": 0.15
    }
  ],
  "steps": [
    { "description": "soften aromatics", "minutes": 12, "active": true },
    { "description": "simmer curry", "minutes": 25, "active": false }
  ]
}
```

## Rules

- **Per-serving quantities.** Every `quantity` is the amount of the
  ingredient's `unit` used **per one base serving**. The planner multiplies by
  the guest count.
- **Honest pricing and nutrition.** `cost_per_unit` and `nutrition_per_unit`
  are quoted per one `unit`. Use realistic values; if nutrition is unknown,
  omit the field (it defaults to zero) and note the gap — do not fabricate.
- **Dietary tags describe what the dish IS.** Only tag what the recipe genuinely
  satisfies. A dish is matched to a constraint iff it carries that tag.
- **Ordered, timed steps.** `steps` run in order (index 0 first); the last step
  ends at the serve time when scheduled. Mark `active: false` for passive time
  (proving, roasting, resting) and `active: true` for hands-on work.
- **Stable id.** `id` is kebab-case and unique within a book; it is how a brief
  pins a recipe.

## Design Checklist

1. Portion sanity: does one base serving look right for a plate?
2. Cost sanity: does `cost_per_serving` land in a believable range?
3. Nutrition sanity: are calories/macros plausible for the portion?
4. Timing: do the steps sum to a realistic total; is passive time marked?
5. Constraints: are the dietary tags defensible ingredient-by-ingredient?
