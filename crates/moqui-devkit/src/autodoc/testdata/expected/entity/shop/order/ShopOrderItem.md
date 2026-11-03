# ShopOrderItem

Up: [shop.order](README.md) · [All artifacts](../../../README.md)

- **Entity:** `shop.order.ShopOrderItem`
- **Defined in:** `component://autodoc-shop/entity/ShopEntities.xml` line 21 (autodoc-shop)
- **Table:** `SHOP_ORDER_ITEM` · **Cache:** false
- **Primary key:** `orderId, orderItemSeqId`

A line of an order.

## Fields

| Field | Type | PK | Not null | Default | Description |
|---|---|---|---|---|---|
| `orderId` | `id` | yes |  |  |  |
| `orderItemSeqId` | `id` | yes |  |  |  |
| `quantity` | `number-decimal` |  |  |  |  |
| `lastUpdatedStamp` | `date-time` |  |  |  |  |

## Relationships

| Type | Related | Title | Key map | Description |
|---|---|---|---|---|
| one | [ShopOrder](ShopOrder.md) |  | `orderId` |  |

```mermaid
erDiagram
    ShopOrderItem {
        id orderId PK, FK
        id orderItemSeqId PK
    }
    ShopOrder {
    }
    ShopOrderItem }o--|| ShopOrder : ""
```

## Used by and uses

```mermaid
flowchart LR
    center[["ShopOrderItem"]]
    subgraph used_by_1["Entities (2)"]
        direction TB
        used_by_1_1["ShopOrder"]
        used_by_1_2["ShopOrderAndItem"]
    end
    used_by_1 --> center
    subgraph uses_1["Entities (1)"]
        direction TB
        uses_1_1["ShopOrder"]
    end
    center --> uses_1
```

### Used by

**Entities** (2)

| Name | Full name | How |
|---|---|---|
| [ShopOrder](ShopOrder.md) | `shop.order.ShopOrder` | relationship |
| [ShopOrderAndItem](ShopOrderAndItem.md) | `shop.order.ShopOrderAndItem` | member-entity in view-member ITM |

### Uses

**Entities** (1)

| Name | Full name | How |
|---|---|---|
| [ShopOrder](ShopOrder.md) | `shop.order.ShopOrder` | relationship |
