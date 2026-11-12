"""A tiny in-memory stock tracker."""


class Inventory:
    def __init__(self):
        self.items = {}

    def add(self, name, quantity, price):
        if quantity < 0:
            raise ValueError("quantity must be non-negative")
        self.items[name] = {"quantity": quantity, "price": price}

    def restock(self, name, quantity):
        item = self.items[name]
        item["quantity"] = quantity

    def sell(self, name, quantity):
        item = self.items[name]
        if quantity > item["quantity"]:
            raise ValueError("not enough stock")
        item["quantity"] -= quantity
        return quantity * item["price"]

    def value(self):
        return sum(i["quantity"] * i["price"] for i in self.items.values())
