// SPDX-License-Identifier: GPL-2.0-only
/* Parts are adapted from the Linux files named below, whose notices are:
 * Copyright (c) 2002-3 Patrick Mochel
 * Copyright (c) 2002-3 Open Source Development Labs
 * Copyright (c) 2006 Greg Kroah-Hartman <gregkh@suse.de>
 * Copyright (c) 2006 Novell, Inc.
 */
/* Linux's device model, following Linux
 * cc9de5d4b5ab6459f5774946aa3195ad51236fa8 drivers/base/core.c and class.c,
 * and lib/devres.c. Stela's owners create and attach devices, and
 * nothing reads sysfs: adding a device only keeps its parent alive, a class
 * is its name and no attribute file exists. */
#include <linux/component.h>
#include <linux/device.h>
#include <linux/err.h>
#include <linux/io.h>
#include <linux/kobject.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/overflow.h>
#include <linux/property.h>
#include <linux/slab.h>
#include <stela/device.h>
#include <stela/catalogue.h>
#include <stela/linux.h>
#include <unsupported.h>

static void device_release(struct kobject *kobj)
{
	struct device *dev = kobj_to_dev(kobj);
	struct stela_linux_device_retirement retired;
	if (!stela_linux_device_release_begin(dev, &retired))
		return;

	if (dev->release)
		dev->release(dev);
	else if (dev->type && dev->type->release)
		dev->type->release(dev);
	else if (dev->class && dev->class->dev_release)
		dev->class->dev_release(dev);
	else
		WARN(1, "a device has no release function\n");
	stela_linux_device_release_end(&retired);
}

static const struct kobj_type device_ktype = {
	.release = device_release,
};

void device_initialize(struct device *dev)
{
	kobject_init(&dev->kobj, &device_ktype);
	INIT_LIST_HEAD(&dev->dma_pools);
	mutex_init(&dev->mutex);
	spin_lock_init(&dev->devres_lock);
	INIT_LIST_HEAD(&dev->devres_head);
}

int dev_set_name(struct device *dev, const char *fmt, ...)
{
	va_list arguments;

	va_start(arguments, fmt);
	int error = kobject_set_name_vargs(&dev->kobj, fmt, arguments);
	va_end(arguments);
	return error;
}

struct device *get_device(struct device *dev)
{
	return dev ? kobj_to_dev(kobject_get(&dev->kobj)) : NULL;
}

void put_device(struct device *dev)
{
	if (dev)
		kobject_put(&dev->kobj);
}

int device_add(struct device *dev)
{
	if (dev->init_name) {
		int error = dev_set_name(dev, "%s", dev->init_name);

		if (error)
			return error;
		dev->init_name = NULL;
	}
	if (!dev_name(dev))
		return -EINVAL;
	struct device *parent = dev->parent;
	get_device(dev);
	get_device(parent);
	int error = stela_linux_device_inherit(dev);
	if (error) {
		put_device(dev);
		put_device(parent);
		return error;
	}
	return 0;
}

void device_del(struct device *dev)
{
	struct device *parent = dev->parent;
	stela_linux_device_close(dev);

	stela_linux_devres_release(dev);
	put_device(dev);
	put_device(parent);
}

int device_register(struct device *dev)
{
	device_initialize(dev);
	return device_add(dev);
}

void device_unregister(struct device *dev)
{
	device_del(dev);
	put_device(dev);
}

static void unmap_device_memory(void *mapping)
{
	iounmap(mapping);
}

/* A write-combining mapping that the driver's detach unmaps, following
 * lib/devres.c. */
void __iomem *devm_ioremap_wc(struct device *dev, resource_size_t offset, resource_size_t size)
{
	void __iomem *mapping = ioremap_wc(offset, size);

	if (mapping && devm_add_action_or_reset(dev, unmap_device_memory, (void *)mapping))
		return NULL;
	return mapping;
}

/* A driver's managed resources are Linux's own drivers/base/devres.c. They
 * are released last first when the driver detaches, as really_probe and
 * device_unbind_cleanup do. */
void stela_linux_devres_release(struct device *dev)
{
	devres_release_all(dev);
}

int dev_err_probe(const struct device *dev, int err, const char *fmt, ...)
{
	va_list arguments;

	va_start(arguments, fmt);
	struct va_format vaf = {.fmt = fmt, .va = &arguments};

	if (err == -EPROBE_DEFER)
		dev_dbg(dev, "error %pe: %pV", ERR_PTR(err), &vaf);
	else
		dev_err(dev, "error %pe: %pV", ERR_PTR(err), &vaf);
	va_end(arguments);
	return err;
}

/* drivers/base/swnode.c: nothing here creates a software node, so no
 * firmware node is one. */
bool is_software_node(const struct fwnode_handle *fwnode)
{
	(void)fwnode;
	return false;
}

/* A device's firmware node, following drivers/base/core.c. */
void device_set_node(struct device *dev, struct fwnode_handle *fwnode)
{
	dev->fwnode = fwnode;
	dev->of_node = to_of_node(fwnode);
}

void device_set_of_node_from_dev(struct device *dev, const struct device *dev2)
{
	of_node_put(dev->of_node);
	dev->of_node = of_node_get(dev2->of_node);
	dev_set_of_node_reused(dev);
}

int device_match_name(struct device *dev, const void *name)
{
	return sysfs_streq(dev_name(dev), name);
}

int device_match_of_node(struct device *dev, const void *np)
{
	return np && dev->of_node == np;
}

/* A bus is its type: Stela's owners match and bind the devices of the buses
 * they own, so registering one records nothing. A Linux driver on a bus, a
 * search of a bus's devices and a walk of a device's children need the
 * driver core's lists, which do not exist. Each is said once and answers as
 * Linux does when it finds nothing to act on: a driver is refused (only MIPI
 * DSI and DP AUX bus drivers register one, and the image holds none), a
 * search finds no device, and a walk visits no child, so the children of a
 * MIPI DSI host that goes away (a DSI panel's encoder torn down) stay
 * registered and keep their host's device, as their references hold it. */
int bus_register(const struct bus_type *bus)
{
	return bus && bus->name ? 0 : -EINVAL;
}

void bus_unregister(const struct bus_type *bus)
{
	(void)bus;
}

int driver_register(struct device_driver *driver)
{
	(void)driver;
	stela_linux_unsupported("a Linux driver on a bus: it is refused");
	return -EOPNOTSUPP;
}

/* Only a driver driver_register took is unregistered, and it takes none. */
void driver_unregister(struct device_driver *driver)
{
	(void)driver;
}

struct device *bus_find_device(const struct bus_type *bus, struct device *start,
			       const void *data, device_match_t match)
{
	(void)bus;
	(void)start;
	(void)data;
	(void)match;
	stela_linux_unsupported("a search of a bus's devices: it finds none");
	return NULL;
}

int device_for_each_child(struct device *parent, void *data, device_iter_t fn)
{
	(void)parent;
	(void)data;
	(void)fn;
	stela_linux_unsupported("a walk of a device's children: they stay registered");
	return 0;
}

/* A class is its name: a device of one is created and attached like any
 * other, and its attribute groups are files nothing reads. Registering a
 * class therefore records nothing, as registering a bus does. A search of a
 * class's devices and of a device's children need the driver core's lists,
 * which do not exist. xe's display code asks for both — the panel's
 * backlight devices and the hardware monitor's children — and a hard trap
 * here ended the boot with an #UD right after the tile line. Answering
 * "none" is what the rest of the environment means by an absent device:
 * the display code then skips what it was looking for, and the engine sets
 * the panel's brightness itself through ACPI once the display runs. */
int class_register(const struct class *class)
{
	return class && class->name ? 0 : -EINVAL;
}

void class_unregister(const struct class *class)
{
	(void)class;
}

struct device *class_find_device(const struct class *class, const struct device *start,
				 const void *data, device_match_t match)
{
	(void)class;
	(void)start;
	(void)data;
	(void)match;
	stela_linux_unsupported("a search of a class's devices: it finds none");
	return NULL;
}

struct device *device_find_child(struct device *parent, const void *data,
				 device_match_t match)
{
	(void)parent;
	(void)data;
	(void)match;
	stela_linux_unsupported("a search of a device's children: it finds none");
	return NULL;
}

struct class *class_create(const char *name)
{
	struct class *class = kzalloc_obj(*class);

	if (!class)
		return ERR_PTR(-ENOMEM);
	class->name = name;
	return class;
}

void class_destroy(const struct class *class)
{
	kfree(class);
}

int class_create_file_ns(const struct class *class, const struct class_attribute *attr,
			 const struct ns_common *ns)
{
	(void)class;
	(void)attr;
	(void)ns;
	return 0;
}

void class_remove_file_ns(const struct class *class, const struct class_attribute *attr,
			  const struct ns_common *ns)
{
	(void)class;
	(void)attr;
	(void)ns;
}

/* A device's attribute files and groups do not exist either. */
int device_create_file(struct device *dev, const struct device_attribute *attr)
{
	(void)dev;
	(void)attr;
	return 0;
}

void device_remove_file(struct device *dev, const struct device_attribute *attr)
{
	(void)dev;
	(void)attr;
}

int devm_device_add_group(struct device *dev, const struct attribute_group *group)
{
	(void)dev;
	(void)group;
	return 0;
}

ssize_t show_class_attr_string(const struct class *class, const struct class_attribute *attr,
			       char *buf)
{
	(void)class;
	return sprintf(buf, "%s\n", container_of(attr, struct class_attribute_string, attr)->str);
}

/* Components join an aggregate device when its master registers. Stela's
 * audio does not bind to GPUs' HDMI codecs through one, and no MEI driver
 * aggregates a GPU's content protection, so none ever does. */
int component_add(struct device *dev, const struct component_ops *ops)
{
	(void)dev;
	(void)ops;
	return 0;
}

int component_add_typed(struct device *dev, const struct component_ops *ops, int subcomponent)
{
	(void)dev;
	(void)ops;
	(void)subcomponent;
	return 0;
}

void component_del(struct device *dev, const struct component_ops *ops)
{
	(void)dev;
	(void)ops;
}

/* Runtime power dependencies, which a bound HDMI audio component and an
 * SR-IOV physical function's virtual functions make. Without the driver
 * core's lists none is kept: the link is refused, said once, and its maker
 * goes on as Linux's does when a link fails (audio's bind fails with
 * -ENOMEM, a virtual function stays unlinked). */
struct device_link *device_link_add(struct device *consumer, struct device *supplier, u32 flags)
{
	(void)consumer;
	(void)supplier;
	(void)flags;
	stela_linux_unsupported("a runtime power dependency between devices: it is refused");
	return NULL;
}

/* A link to delete is one device_link_add gave, and it gives none. */
STELA_UNSUPPORTED(void, device_link_del, struct device_link *)

/* No link exists to remove: adding one is what is unsupported. */
void device_link_remove(void *consumer, struct device *supplier)
{
	(void)consumer;
	(void)supplier;
}

#ifdef STELA_LINUX_DEVICE_TEST
#include <stela/linux.h>
#include <stela/test.h>

static unsigned int released;
static char undone[4];
static unsigned int undone_count;

static void release(struct device *dev)
{
	(void)dev;
	released++;
}

static void undo(void *data)
{
	undone[undone_count++] = *(const char *)data;
}

static bool test_property_bool(const struct fwnode_handle *node, const char *name)
{
	return !node->secondary && !strcmp(name, "test-enabled");
}

static int test_property_int(const struct fwnode_handle *node, const char *name,
			     unsigned int size, void *out, size_t count)
{
	if (node->secondary || strcmp(name, "test-values"))
		return -EINVAL;

	u32 *values = out;
	assert(values && size == sizeof(*values) && count == 2);
	values[0] = 7;
	values[1] = 13;
	return 0;
}

static u8 window[PAGE_SIZE] __aligned(PAGE_SIZE);
static unsigned int mapped;

static void *map_device(void *context, u64 physical, size_t size, bool write_combining)
{
	(void)context;
	assert(physical == 0xd0000000 && size == sizeof(window) && write_combining);
	mapped++;
	return window;
}

static void unmap_device(void *context, void *address)
{
	(void)context;
	assert(address == window);
	mapped--;
}

static ssize_t test_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	(void)dev;
	(void)attr;
	(void)buf;
	return 0;
}

static void test_properties(void)
{
	static const struct fwnode_operations ops = {
		.property_read_bool = test_property_bool,
		.property_read_int_array = test_property_int,
	};
	struct fwnode_handle secondary = {.ops = &ops};
	struct fwnode_handle primary = {.ops = &ops, .secondary = &secondary};
	const struct device dev = {.fwnode = &primary};
	u32 values[2] = {};

	assert(dev_fwnode(&dev) == &primary);
	assert(device_property_read_bool(&dev, "test-enabled"));
	assert(!device_property_read_bool(&dev, "absent"));
	assert(!device_property_read_u32_array(&dev, "test-values", values,
					       ARRAY_SIZE(values)));
	assert(values[0] == 7 && values[1] == 13);
}

static const struct stela_linux_services services = {
	.map_device = map_device,
	.unmap_device = unmap_device,
};
static const struct stela_linux_owner owner = {.services = &services};

int main(void)
{
	stela_linux_enter(&owner);
	test_properties();
	struct device dev = {.release = release};
	device_initialize(&dev);
	assert(get_device(&dev) == &dev && !get_device(NULL));
	put_device(&dev);
	assert(!released);
	put_device(NULL);
	put_device(&dev);
	assert(released == 1);

	/* A child keeps its parent; managed actions undo last first. */
	released = 0;
	struct device parent = {.release = release};
	struct device child = {.parent = &parent, .release = release};
	device_initialize(&parent);
	assert(device_register(&child) == -EINVAL);
	assert(!dev_set_name(&child, "child") && !device_add(&child));
	put_device(&parent);
	assert(!released);

	static const char a = 'a', b = 'b', c = 'c';
	assert(!__devm_add_action(&child, undo, (void *)&a, "a"));
	assert(!__devm_add_action(&child, undo, (void *)&b, "b"));
	assert(!__devm_add_action(&child, undo, (void *)&c, "c"));
	devm_release_action(&child, undo, (void *)&b);
	assert(undone_count == 1 && undone[0] == 'b');
	/* An action taken back does not run; memory is the device's until it
	 * is freed or the device goes, and a mapping is unmapped then. */
	static const char d = 'd';
	assert(!__devm_add_action(&child, undo, (void *)&d, "d"));
	assert(!devm_remove_action_nowarn(&child, undo, (void *)&d));
	assert(devm_remove_action_nowarn(&child, undo, (void *)&d) == -ENOENT);
	u8 *zeroed = devm_kzalloc(&child, 24, GFP_KERNEL);
	u8 *kept = devm_kmalloc(&child, 8, GFP_KERNEL);
	assert(zeroed && kept && !zeroed[0] && !zeroed[23]);
	assert(IS_ALIGNED((unsigned long)zeroed, ARCH_DMA_MINALIGN));
	assert(devm_kmalloc(&child, 0, GFP_KERNEL) == ZERO_SIZE_PTR);
	assert(!devm_kmalloc(&child, SIZE_MAX, GFP_KERNEL));
	devm_kfree(&child, zeroed);
	devm_kfree(&child, NULL);
	assert(devm_ioremap_wc(&child, 0xd0000000, PAGE_SIZE) == (void __iomem *)window);
	assert(mapped == 1);
	static DEVICE_ATTR_RO(test);
	assert(!device_create_file(&child, &dev_attr_test));
	device_remove_file(&child, &dev_attr_test);
	assert(!component_add_typed(&child, NULL, 1));
	device_unregister(&child);
	assert(!memcmp(undone, "bca", 3) && undone_count == 3 && !mapped);
	put_device(&child);
	assert(released == 2);

	/* A node set on a device is what a match by node compares. */
	struct fwnode_handle node = {};
	struct device with_node = {}, other = {};
	device_set_node(&with_node, &node);
	assert(with_node.fwnode == &node && !with_node.of_node);
	device_set_of_node_from_dev(&other, &with_node);
	assert(!other.of_node && dev_of_node_reused(&other));
	assert(!device_match_of_node(&other, NULL));
	struct device named = {};
	device_initialize(&named);
	assert(!dev_set_name(&named, "hwmon%d", 3));
	assert(device_match_name(&named, "hwmon3\n") && !device_match_name(&named, "hwmon"));
	kfree_const(named.kobj.name);
	static const struct bus_type test_bus = {.name = "test"};
	static const struct bus_type unnamed = {};
	assert(!bus_register(&test_bus) && bus_register(&unnamed) == -EINVAL);
	bus_unregister(&test_bus);

	static const struct class test_class = {.name = "test"};
	static const struct class unnamed_class = {};
	assert(!class_register(&test_class) && class_register(&unnamed_class) == -EINVAL);
	class_unregister(&test_class);

	struct class *class = class_create("test");
	assert(!IS_ERR(class) && !strcmp(class->name, "test"));
	class_destroy(class);
	stela_linux_exit();
	printf("Linux device references: the last put releases the device passed\n");
	return 0;
}
#endif
