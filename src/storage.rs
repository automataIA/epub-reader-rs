use crate::models::{Annotation, ReaderLocation, ReaderPreferences, StoredBook};
use gloo_storage::{LocalStorage, Storage};
use rexie::{ObjectStore, Rexie, TransactionMode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsValue;

const DB_NAME: &str = "epub-reader-rs";
const DB_VERSION: u32 = 2;
const STORE_BOOKS: &str = "books";
const STORE_LOCATIONS: &str = "locations";
const STORE_ANNOTATIONS: &str = "annotations";
const STORE_PREFERENCES: &str = "preferences";
const PREF_KEY: &str = "reader";
const PREF_LOCAL_KEY: &str = "epub-reader-rs.preferences";

pub struct AppStorage {
    db: Rexie,
}

impl AppStorage {
    pub async fn open() -> Result<Self, String> {
        let db = Rexie::builder(DB_NAME)
            .version(DB_VERSION)
            .add_object_store(ObjectStore::new(STORE_BOOKS))
            .add_object_store(ObjectStore::new(STORE_LOCATIONS))
            .add_object_store(ObjectStore::new(STORE_ANNOTATIONS))
            .add_object_store(ObjectStore::new(STORE_PREFERENCES))
            .build()
            .await
            .map_err(|err| format!("{err:?}"))?;
        Ok(Self { db })
    }

    pub async fn list_books(&self) -> Result<Vec<StoredBook>, String> {
        self.get_all(STORE_BOOKS).await
    }

    pub async fn save_book(&self, book: &StoredBook) -> Result<(), String> {
        self.put(STORE_BOOKS, &book.record.id, book).await
    }

    pub async fn save_location(&self, location: &ReaderLocation) -> Result<(), String> {
        self.put(STORE_LOCATIONS, &location.book_id, location).await
    }

    pub async fn get_location(&self, book_id: &str) -> Result<Option<ReaderLocation>, String> {
        self.get(STORE_LOCATIONS, book_id).await
    }

    pub async fn list_annotations(&self, book_id: &str) -> Result<Vec<Annotation>, String> {
        let annotations: Vec<Annotation> = self.get_all(STORE_ANNOTATIONS).await?;
        Ok(annotations
            .into_iter()
            .filter(|annotation| annotation.book_id == book_id)
            .collect())
    }

    pub async fn save_annotation(&self, annotation: &Annotation) -> Result<(), String> {
        self.put(STORE_ANNOTATIONS, &annotation.id, annotation)
            .await
    }

    pub async fn save_preferences(&self, preferences: &ReaderPreferences) -> Result<(), String> {
        let _ = LocalStorage::set(PREF_LOCAL_KEY, preferences);
        self.put(STORE_PREFERENCES, PREF_KEY, preferences).await
    }

    pub async fn get_preferences(&self) -> ReaderPreferences {
        match self.get(STORE_PREFERENCES, PREF_KEY).await {
            Ok(Some(preferences)) => preferences,
            _ => LocalStorage::get(PREF_LOCAL_KEY).unwrap_or_default(),
        }
    }

    async fn put<T>(&self, store_name: &str, key: &str, value: &T) -> Result<(), String>
    where
        T: Serialize,
    {
        let transaction = self
            .db
            .transaction(&[store_name], TransactionMode::ReadWrite)
            .map_err(|err| format!("{err:?}"))?;
        let store = transaction
            .store(store_name)
            .map_err(|err| format!("{err:?}"))?;
        let value = serde_wasm_bindgen::to_value(value).map_err(|err| err.to_string())?;
        let key = JsValue::from_str(key);
        store
            .put(&value, Some(&key))
            .await
            .map_err(|err| format!("{err:?}"))?;
        transaction.done().await.map_err(|err| format!("{err:?}"))?;
        Ok(())
    }

    async fn get<T>(&self, store_name: &str, key: &str) -> Result<Option<T>, String>
    where
        T: DeserializeOwned,
    {
        let transaction = self
            .db
            .transaction(&[store_name], TransactionMode::ReadOnly)
            .map_err(|err| format!("{err:?}"))?;
        let store = transaction
            .store(store_name)
            .map_err(|err| format!("{err:?}"))?;
        let key = JsValue::from_str(key);
        let value = store.get(key).await.map_err(|err| format!("{err:?}"))?;
        transaction.done().await.map_err(|err| format!("{err:?}"))?;

        value
            .map(serde_wasm_bindgen::from_value)
            .transpose()
            .map_err(|err| err.to_string())
    }

    async fn get_all<T>(&self, store_name: &str) -> Result<Vec<T>, String>
    where
        T: DeserializeOwned,
    {
        let transaction = self
            .db
            .transaction(&[store_name], TransactionMode::ReadOnly)
            .map_err(|err| format!("{err:?}"))?;
        let store = transaction
            .store(store_name)
            .map_err(|err| format!("{err:?}"))?;
        let values = store
            .get_all(None, None)
            .await
            .map_err(|err| format!("{err:?}"))?;
        transaction.done().await.map_err(|err| format!("{err:?}"))?;

        values
            .into_iter()
            .map(serde_wasm_bindgen::from_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| err.to_string())
    }
}
