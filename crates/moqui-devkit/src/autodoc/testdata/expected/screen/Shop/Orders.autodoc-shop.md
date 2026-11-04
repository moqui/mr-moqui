# Orders

Up: [Shop](README.md) · [All artifacts](../../README.md)

- **Screen:** `component://autodoc-shop/screen/Shop/Orders.xml`
- **Menu title:** Orders
- **Render modes:** all · **Requires login:** true

The orders of the shop, with Create order.

## Parameters

| Parameter | Required | From |
|---|---|---|
| `statusId` |  |  |

## Transitions

| Transition | Method | Service | Response |
|---|---|---|---|
| `createOrder` | any | [`shop.OrderServices.create#Order`](../../service/shop/OrderServices/create-Order.md) | `.` |

## Forms

| Form | Type | Fields |
|---|---|---|
| `OrderList` | form-list | orderId, grandTotal |

## Used by and uses

```mermaid
flowchart LR
    center[["Orders"]]
    subgraph used_by_1["Screens (1)"]
        direction TB
        used_by_1_1["Shop"]
    end
    used_by_1 --> center
    subgraph uses_1["Services (1)"]
        direction TB
        uses_1_1["create#35;Order"]
    end
    center --> uses_1
    subgraph uses_2["Entities (1)"]
        direction TB
        uses_2_1["ShopOrder"]
    end
    center --> uses_2
```

### Used by

**Screens** (1)

| Name | Full name | How |
|---|---|---|
| [Shop](../Shop.md) | `component://autodoc-shop/screen/Shop.xml` | subscreens-item in subscreen Orders |

### Uses

**Services** (1)

| Name | Full name | How |
|---|---|---|
| [create#Order](../../service/shop/OrderServices/create-Order.md) | `shop.OrderServices.create#Order` | service-call |

**Entities** (1)

| Name | Full name | How |
|---|---|---|
| [ShopOrder](../../entity/shop/order/ShopOrder.md) | `shop.order.ShopOrder` | econdition in field statusId, entity-find |
