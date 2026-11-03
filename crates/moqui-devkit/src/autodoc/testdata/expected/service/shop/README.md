# Services in `shop`

Up: [Services](../README.md) · [All artifacts](../../README.md)

## Service files

| File | Services | Component |
|---|---|---|
| [OrderServices](OrderServices.md) | 3 | autodoc-shop |

## Files, services and interfaces

```mermaid
classDiagram
    class OrderServices["OrderServices"] {
        +define_OrderInfo()
        +create_Order()
        +get_Order()
    }
    class define_OrderInfo["define#35;OrderInfo"] {
        <<interface>>
    }
    define_OrderInfo <|.. OrderServices
```
