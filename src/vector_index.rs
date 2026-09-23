use crate::simd::dot_product;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct VectorIndex {
    ids: Vec<String>,
    scopes: Vec<Option<String>>,
    tags: Vec<Vec<String>>,
    embeddings: Vec<f32>,
    id_map: HashMap<String, usize>,
    dim: usize,
}

impl VectorIndex {
    pub fn new() -> Self {
        Self {
            ids: Vec::new(),
            scopes: Vec::new(),
            tags: Vec::new(),
            embeddings: Vec::new(),
            id_map: HashMap::new(),
            dim: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.scopes.clear();
        self.tags.clear();
        self.embeddings.clear();
        self.id_map.clear();
        self.dim = 0;
    }

    pub fn insert_or_update(
        &mut self,
        id: &str,
        scope: Option<&str>,
        entry_tags: &[String],
        embedding: &[f32],
    ) {
        if embedding.is_empty() {
            return;
        }

        if self.dim == 0 {
            self.dim = embedding.len();
        } else if self.dim != embedding.len() {
            // Dimension mismatch, skip
            return;
        }

        if let Some(&index) = self.id_map.get(id) {
            // Update existing entry
            self.scopes[index] = scope.map(str::to_owned);
            self.tags[index] = entry_tags.to_vec();
            let offset = index * self.dim;
            self.embeddings[offset..offset + self.dim].copy_from_slice(embedding);
        } else {
            // Append new entry
            let index = self.ids.len();
            self.ids.push(id.to_owned());
            self.scopes.push(scope.map(str::to_owned));
            self.tags.push(entry_tags.to_vec());
            self.embeddings.extend_from_slice(embedding);
            self.id_map.insert(id.to_owned(), index);
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let Some(&index) = self.id_map.get(id) else {
            return false;
        };

        let last_index = self.ids.len() - 1;
        if index != last_index {
            // Swap remove with last element to maintain contiguous layout
            let last_id = self.ids[last_index].clone();
            self.ids.swap(index, last_index);
            self.scopes.swap(index, last_index);
            self.tags.swap(index, last_index);

            // Swap embedding slice
            let offset = index * self.dim;
            let last_offset = last_index * self.dim;
            for d in 0..self.dim {
                self.embeddings.swap(offset + d, last_offset + d);
            }

            self.id_map.insert(last_id, index);
        }

        self.ids.pop();
        self.scopes.pop();
        self.tags.pop();
        self.embeddings.truncate(self.embeddings.len() - self.dim);
        self.id_map.remove(id);

        if self.ids.is_empty() {
            self.dim = 0;
        }

        true
    }

    /// Search the index using vector dot product.
    /// Returns matching (entry_id, similarity_score) sorted descending by score.
    pub fn search(
        &self,
        query: &[f32],
        filter_scope: Option<&str>,
        filter_tags: &[String],
        min_score: f32,
        limit: usize,
    ) -> Vec<(String, f32)> {
        if self.ids.is_empty() || self.dim == 0 || query.len() != self.dim {
            return Vec::new();
        }

        let mut matches = Vec::new();
        for (i, id) in self.ids.iter().enumerate() {
            // Filter by project scope if specified
            if let Some(req_scope) = filter_scope {
                if self.scopes[i].as_deref() != Some(req_scope) {
                    continue;
                }
            }

            // Filter by tags (must contain all required tags)
            if !filter_tags.is_empty() {
                let entry_tags = &self.tags[i];
                let has_all_tags = filter_tags
                    .iter()
                    .all(|req_tag| entry_tags.iter().any(|t| t.eq_ignore_ascii_case(req_tag)));
                if !has_all_tags {
                    continue;
                }
            }

            let offset = i * self.dim;
            let vec_slice = &self.embeddings[offset..offset + self.dim];
            let score = dot_product(query, vec_slice);

            if score >= min_score {
                matches.push((id.clone(), score));
            }
        }

        matches.sort_by(|a, b| b.1.total_cmp(&a.1));
        if matches.len() > limit {
            matches.truncate(limit);
        }

        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_index_insert_search_remove() {
        let mut index = VectorIndex::new();
        let e1 = vec![1.0, 0.0, 0.0];
        let e2 = vec![0.0, 1.0, 0.0];
        let e3 = vec![0.707, 0.707, 0.0];

        index.insert_or_update("doc1", Some("backend"), &["rust".to_string()], &e1);
        index.insert_or_update("doc2", Some("frontend"), &["ts".to_string()], &e2);
        index.insert_or_update(
            "doc3",
            Some("backend"),
            &["rust".to_string(), "api".to_string()],
            &e3,
        );

        assert_eq!(index.len(), 3);

        // Query near doc1
        let results = index.search(&e1, None, &[], 0.0, 5);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, "doc1");
        assert!((results[0].1 - 1.0).abs() < 1e-4);

        // Filter by project scope
        let backend_results = index.search(&e1, Some("backend"), &[], 0.0, 5);
        assert_eq!(backend_results.len(), 2);
        assert_eq!(backend_results[0].0, "doc1");
        assert_eq!(backend_results[1].0, "doc3");

        // Remove doc1
        assert!(index.remove("doc1"));
        assert_eq!(index.len(), 2);
        let after_remove = index.search(&e1, None, &[], 0.0, 5);
        assert_eq!(after_remove.len(), 2);
        assert_ne!(after_remove[0].0, "doc1");
    }
}
