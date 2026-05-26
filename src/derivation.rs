use serde::Deserialize;
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::l2::L2Store;
use crate::l3::{L3Store, Link};
use crate::session::ReadFilter;

#[derive(Debug, Error)]
pub enum DerivationError {
    #[error("db: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("payload JSON invalide: {0}")]
    Payload(String),
    #[error("temps: {0}")]
    Time(String),
    #[error("aucune page active — créez-en une avec `links create-page`")]
    NoActivePage,
}

pub trait DerivationRule {
    /// Identifiant stable de la règle, stocké en `derived_by`.
    fn name(&self) -> &'static str;

    /// Applique la règle. Retourne le nombre de liens créés.
    fn apply(&self, l2: &L2Store, l3: &mut L3Store) -> Result<usize, DerivationError>;
}

/// Règle : pour chaque fait validé de type "recipe", crée un lien vers
/// chaque ingrédient comme "shopping_item", rattaché à la page active.
pub struct RecipeToShopping;

#[derive(Debug, Deserialize)]
struct RecipePayload {
    #[serde(default)]
    ingredients: Vec<String>,
}

impl DerivationRule for RecipeToShopping {
    fn name(&self) -> &'static str {
        "rule:recipe-to-shopping"
    }

    fn apply(&self, l2: &L2Store, l3: &mut L3Store) -> Result<usize, DerivationError> {
        let active_page = l3
            .active_page()?
            .ok_or(DerivationError::NoActivePage)?;

        let recipes = l2.list_by_type("recipe", ReadFilter::All)?;
        let mut created = 0;
        for fact in recipes {
            // Idempotence : on saute si la règle a déjà tourné sur ce fact.
            if l3.exists_derived("fact", &fact.id, "derives_to", self.name())? {
                continue;
            }
            let payload: RecipePayload = serde_json::from_str(&fact.payload)
                .map_err(|e| DerivationError::Payload(e.to_string()))?;
            for ingredient in &payload.ingredients {
                let now = OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .map_err(|e| DerivationError::Time(e.to_string()))?;
                let item_id = Uuid::new_v4().to_string();
                // Lien : recette → shopping_item
                l3.add_link(&Link {
                    id: Uuid::new_v4().to_string(),
                    src_kind: "fact".into(),
                    src_id: fact.id.clone(),
                    dst_kind: "shopping_item".into(),
                    dst_id: item_id.clone(),
                    rel_type: "derives_to".into(),
                    derived_by: self.name().into(),
                    metadata: Some(format!(r#"{{"label":"{}"}}"#, escape_json(ingredient))),
                    created_at: now.clone(),
                })?;
                // Lien : shopping_item → page active (belongs_to)
                l3.add_link(&Link {
                    id: Uuid::new_v4().to_string(),
                    src_kind: "shopping_item".into(),
                    src_id: item_id,
                    dst_kind: "page".into(),
                    dst_id: active_page.id.clone(),
                    rel_type: "belongs_to".into(),
                    derived_by: self.name().into(),
                    metadata: None,
                    created_at: now,
                })?;
                created += 2;
            }
        }
        Ok(created)
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Liste des règles disponibles, dans l'ordre où `--rule all` les applique.
pub fn all_rules() -> Vec<Box<dyn DerivationRule>> {
    vec![Box::new(RecipeToShopping)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::l2::{Fact, L2Store};
    use crate::l3::{L3Store, Page};
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("az-deriv-test-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    fn recipe(id: &str, ingredients: &[&str]) -> Fact {
        let payload = format!(
            r#"{{"ingredients":[{}]}}"#,
            ingredients
                .iter()
                .map(|s| format!(r#""{s}""#))
                .collect::<Vec<_>>()
                .join(",")
        );
        Fact {
            id: id.into(),
            version: 1,
            fact_type: "recipe".into(),
            payload,
            block_id: None,
            sensitivity: false,
            created_at: "2026-05-26T10:00:00Z".into(),
            validated_at: Some("2026-05-26T10:01:00Z".into()),
        }
    }

    #[test]
    fn no_active_page_errors() {
        let path = tmp("no_page");
        let l2 = L2Store::open(&path, &db::test_key()).unwrap();
        let mut l3 = L3Store::open(&path, &db::test_key()).unwrap();
        let err = RecipeToShopping.apply(&l2, &mut l3).unwrap_err();
        assert!(matches!(err, DerivationError::NoActivePage));
    }

    #[test]
    fn recipe_to_shopping_creates_items() {
        let path = tmp("recipe_create");
        let mut l2 = L2Store::open(&path, &db::test_key()).unwrap();
        l2.insert(&recipe("r1", &["œufs", "farine", "lait"]), &[])
            .unwrap();
        let mut l3 = L3Store::open(&path, &db::test_key()).unwrap();
        l3.add_page(&Page {
            id: "p1".into(),
            title: "Courses".into(),
            description: None,
            is_active: true,
            created_at: "2026-05-26T10:00:00Z".into(),
            archived_at: None,
        })
        .unwrap();

        let n = RecipeToShopping.apply(&l2, &mut l3).unwrap();
        // 3 ingrédients × 2 liens (recipe→item + item→page) = 6 liens
        assert_eq!(n, 6);
        let outgoing = l3.list_outgoing("fact", "r1").unwrap();
        assert_eq!(outgoing.len(), 3);
        assert!(outgoing.iter().all(|l| l.dst_kind == "shopping_item"));
        let incoming = l3.list_incoming("page", "p1").unwrap();
        assert_eq!(incoming.len(), 3);
    }

    #[test]
    fn recipe_to_shopping_is_idempotent() {
        let path = tmp("recipe_idem");
        let mut l2 = L2Store::open(&path, &db::test_key()).unwrap();
        l2.insert(&recipe("r1", &["œufs"]), &[]).unwrap();
        let mut l3 = L3Store::open(&path, &db::test_key()).unwrap();
        l3.add_page(&Page {
            id: "p1".into(),
            title: "C".into(),
            description: None,
            is_active: true,
            created_at: "2026-05-26T10:00:00Z".into(),
            archived_at: None,
        })
        .unwrap();
        assert_eq!(RecipeToShopping.apply(&l2, &mut l3).unwrap(), 2);
        assert_eq!(
            RecipeToShopping.apply(&l2, &mut l3).unwrap(),
            0,
            "deuxième passage doit être no-op"
        );
    }

    #[test]
    fn non_recipe_facts_are_ignored() {
        let path = tmp("non_recipe");
        let mut l2 = L2Store::open(&path, &db::test_key()).unwrap();
        let f = Fact {
            id: "x".into(),
            version: 1,
            fact_type: "transaction".into(),
            payload: r#"{"amount":50}"#.into(),
            block_id: None,
            sensitivity: false,
            created_at: "2026-05-26T10:00:00Z".into(),
            validated_at: None,
        };
        l2.insert(&f, &[]).unwrap();
        let mut l3 = L3Store::open(&path, &db::test_key()).unwrap();
        l3.add_page(&Page {
            id: "p".into(),
            title: "C".into(),
            description: None,
            is_active: true,
            created_at: "2026-05-26T10:00:00Z".into(),
            archived_at: None,
        })
        .unwrap();
        assert_eq!(RecipeToShopping.apply(&l2, &mut l3).unwrap(), 0);
    }
}
