import unittest

from inventory import Inventory


class InventoryTest(unittest.TestCase):
    def test_sell_reduces_stock_and_returns_revenue(self):
        inv = Inventory()
        inv.add("apple", 10, 0.5)
        self.assertEqual(inv.sell("apple", 4), 2.0)
        self.assertEqual(inv.items["apple"]["quantity"], 6)

    def test_value_sums_all_items(self):
        inv = Inventory()
        inv.add("apple", 10, 0.5)
        inv.add("pear", 2, 1.25)
        self.assertEqual(inv.value(), 7.5)


if __name__ == "__main__":
    unittest.main()
