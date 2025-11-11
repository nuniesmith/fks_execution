//! Plugin Registry
//!
//! Manages multiple execution plugins and routes orders to the appropriate backend

use super::{ExecutionPlugin, ExecutionResult, MarketData, Order};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Plugin registry for managing multiple execution backends
pub struct PluginRegistry {
    plugins: Arc<RwLock<HashMap<String, Arc<dyn ExecutionPlugin>>>>,
    default_plugin: Arc<RwLock<Option<String>>>,
}

impl PluginRegistry {
    /// Create a new plugin registry
    pub fn new() -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            default_plugin: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Register a plugin
    ///
    /// # Arguments
    /// * `name` - Unique name for the plugin
    /// * `plugin` - Plugin instance
    pub async fn register(&self, name: String, plugin: Arc<dyn ExecutionPlugin>) {
        let mut plugins = self.plugins.write().await;
        plugins.insert(name.clone(), plugin);
        
        // Set as default if first plugin
        let mut default = self.default_plugin.write().await;
        if default.is_none() {
            *default = Some(name);
        }
    }
    
    /// Set the default plugin
    pub async fn set_default(&self, name: String) -> Result<(), String> {
        let plugins = self.plugins.read().await;
        if !plugins.contains_key(&name) {
            return Err(format!("Plugin '{}' not found", name));
        }
        
        let mut default = self.default_plugin.write().await;
        *default = Some(name);
        Ok(())
    }
    
    /// Get a plugin by name
    pub async fn get(&self, name: &str) -> Option<Arc<dyn ExecutionPlugin>> {
        let plugins = self.plugins.read().await;
        plugins.get(name).cloned()
    }
    
    /// Get the default plugin
    pub async fn get_default(&self) -> Option<Arc<dyn ExecutionPlugin>> {
        let default_name = {
            let default = self.default_plugin.read().await;
            default.clone()?
        };
        
        self.get(&default_name).await
    }
    
    /// Execute order using specified plugin or default
    pub async fn execute_order(
        &self,
        order: Order,
        plugin_name: Option<&str>,
    ) -> Result<ExecutionResult, Box<dyn std::error::Error + Send + Sync>> {
        let plugin = if let Some(name) = plugin_name {
            self.get(name).await
                .ok_or_else(|| format!("Plugin '{}' not found", name))?
        } else {
            self.get_default().await
                .ok_or("No default plugin configured")?
        };
        
        plugin.execute_order(order).await
    }
    
    /// Fetch market data using specified plugin or default
    pub async fn fetch_data(
        &self,
        symbol: &str,
        plugin_name: Option<&str>,
    ) -> Result<MarketData, Box<dyn std::error::Error + Send + Sync>> {
        let plugin = if let Some(name) = plugin_name {
            self.get(name).await
                .ok_or_else(|| format!("Plugin '{}' not found", name))?
        } else {
            self.get_default().await
                .ok_or("No default plugin configured")?
        };
        
        plugin.fetch_data(symbol).await
    }
    
    /// List all registered plugins
    pub async fn list_plugins(&self) -> Vec<String> {
        let plugins = self.plugins.read().await;
        plugins.keys().cloned().collect()
    }
    
    /// Health check all plugins
    pub async fn health_check_all(&self) -> HashMap<String, bool> {
        let plugins = self.plugins.read().await;
        let mut results = HashMap::new();
        
        for (name, plugin) in plugins.iter() {
            let health = plugin.health_check().await.unwrap_or(false);
            results.insert(name.clone(), health);
        }
        
        results
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::mock::MockPlugin;
    use crate::plugins::{OrderSide, OrderType};
    
    #[tokio::test]
    async fn test_registry_register_and_get() {
        let registry = PluginRegistry::new();
        
        let mut mock_plugin = MockPlugin::new("mock1");
        mock_plugin.init(serde_json::json!({})).await.unwrap();
        
        registry.register("mock1".to_string(), Arc::new(mock_plugin)).await;
        
        let plugin = registry.get("mock1").await;
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().name(), "mock1");
    }
    
    #[tokio::test]
    async fn test_registry_default_plugin() {
        let registry = PluginRegistry::new();
        
        let mut mock1 = MockPlugin::new("mock1");
        mock1.init(serde_json::json!({})).await.unwrap();
        
        let mut mock2 = MockPlugin::new("mock2");
        mock2.init(serde_json::json!({})).await.unwrap();
        
        registry.register("mock1".to_string(), Arc::new(mock1)).await;
        registry.register("mock2".to_string(), Arc::new(mock2)).await;
        
        // First registered should be default
        let default = registry.get_default().await.unwrap();
        assert_eq!(default.name(), "mock1");
        
        // Change default
        registry.set_default("mock2".to_string()).await.unwrap();
        let default = registry.get_default().await.unwrap();
        assert_eq!(default.name(), "mock2");
    }
    
    #[tokio::test]
    async fn test_registry_execute_order() {
        let registry = PluginRegistry::new();
        
        let mut mock_plugin = MockPlugin::new("mock1");
        mock_plugin.init(serde_json::json!({})).await.unwrap();
        registry.register("mock1".to_string(), Arc::new(mock_plugin)).await;
        
        let order = Order {
            symbol: "BTC/USDT".to_string(),
            side: OrderSide::Buy,
            order_type: OrderType::Market,
            quantity: 0.1,
            price: Some(67500.0),
            stop_loss: None,
            take_profit: None,
            confidence: 0.75,
        };
        
        // Execute with default plugin
        let result = registry.execute_order(order.clone(), None).await.unwrap();
        assert!(result.success);
        
        // Execute with named plugin
        let result = registry.execute_order(order, Some("mock1")).await.unwrap();
        assert!(result.success);
    }
    
    #[tokio::test]
    async fn test_registry_list_plugins() {
        let registry = PluginRegistry::new();
        
        let mut mock1 = MockPlugin::new("mock1");
        mock1.init(serde_json::json!({})).await.unwrap();
        
        let mut mock2 = MockPlugin::new("mock2");
        mock2.init(serde_json::json!({})).await.unwrap();
        
        registry.register("mock1".to_string(), Arc::new(mock1)).await;
        registry.register("mock2".to_string(), Arc::new(mock2)).await;
        
        let plugins = registry.list_plugins().await;
        assert_eq!(plugins.len(), 2);
        assert!(plugins.contains(&"mock1".to_string()));
        assert!(plugins.contains(&"mock2".to_string()));
    }
    
    #[tokio::test]
    async fn test_registry_health_check_all() {
        let registry = PluginRegistry::new();
        
        let mut mock1 = MockPlugin::new("mock1");
        mock1.init(serde_json::json!({})).await.unwrap();
        
        let mut mock2 = MockPlugin::new("mock2");
        mock2.init(serde_json::json!({})).await.unwrap();
        
        registry.register("mock1".to_string(), Arc::new(mock1)).await;
        registry.register("mock2".to_string(), Arc::new(mock2)).await;
        
        let health = registry.health_check_all().await;
        
        assert_eq!(health.len(), 2);
        assert_eq!(health.get("mock1"), Some(&true));
        assert_eq!(health.get("mock2"), Some(&true));
    }
}
